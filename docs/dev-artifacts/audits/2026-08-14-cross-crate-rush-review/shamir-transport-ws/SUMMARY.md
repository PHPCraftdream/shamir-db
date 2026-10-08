<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-transport-ws — SUMMARY independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

All 49 summary claims and 16 plans were independently checked. Source mechanisms generally persist, but the ledger conflates optional behavior with defects and contains stale verification qualifications. Browser subprotocol failure is normatively established, the Pong witness is supported through the locked TLS stack, and additional project-contract omissions remain.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 49 | 45 | 0 | 0 | 3 | 0 | 1 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1-1"></a>

### Claim 1.1 — `accept_browser_ws` — the spec §9 Origin enforcement path — has zero test coverage

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

Conditional TS tests reach the browser-profile production handshake and authentication. Their positive oracle cannot catch deleting Origin rejection.

Evidence: [crates/shamir-client-ts/src/__tests__/connect.test.ts:203](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-ts/src/__tests__/connect.test.ts#L203); [.github/workflows/ts-e2e-nightly.yml:85](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/.github/workflows/ts-e2e-nightly.yml#L85).

Grouping/duplicate: [correctness-tdd.md#1](correctness-tdd.md#review-1). This is not an additional independent defect.

<a id="review-1-2"></a>

### Claim 1.2 — Framing's malformed-input error paths untested; `rejects_oversized_frame` is under-asserted

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Bare is_err does not pin TooLarge; registered tests do not assert malformed-prefix, short-binary, Text, Close or EOF variants.

Evidence: [crates/shamir-transport-ws/tests/framing_round_trip.rs:69](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/tests/framing_round_trip.rs#L69); [crates/shamir-transport-ws/src/framing.rs:148](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/framing.rs#L148).

Grouping/duplicate: [correctness-tdd.md#2](correctness-tdd.md#review-2). This is not an additional independent defect.

<a id="review-1-3"></a>

### Claim 1.3 — `WsAcceptError::WrongPath` never constructed; `OriginRejected(#[from])` unreachable

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Both acceptors convert callback rejection to Handshake(Error::Http), never their typed policy variants.

Evidence: [crates/shamir-transport-ws/src/server.rs:99](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/server.rs#L99); [crates/shamir-transport-ws/src/server.rs:144](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/server.rs#L144).

Grouping/duplicate: [error-handling-lifecycle.md#1](error-handling-lifecycle.md#review-1). This is not an additional independent defect.

<a id="review-1-4"></a>

### Claim 1.4 — `ws_send_sink` has no send-side cap; `payload.len() as u32` silently truncates

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

No guard precedes allocation/narrowing. Corruption starts at 2^32 bytes; normal over-16-MiB sends are already unsupported.

Evidence: [crates/shamir-transport-ws/src/framing.rs:118](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/framing.rs#L118).

Grouping/duplicate: [api-wire-protocol.md#3](api-wire-protocol.md#review-3). This is not an additional independent defect.

<a id="review-1-5"></a>

### Claim 1.5 — Wildcard origin matcher is raw string logic — accepts literal `*` and userinfo forms; unvalidated patterns fail silently

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The first-dot suffix comparison accepts both cited raw malformed strings. Browsers do not normally serialize those forms; configuration validation remains absent.

Evidence: [crates/shamir-transport-ws/src/browser.rs:37](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/browser.rs#L37); [crates/shamir-transport-ws/src/browser.rs:70](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/browser.rs#L70).

Grouping/duplicate: [correctness-tdd.md#5](correctness-tdd.md#review-5). This is not an additional independent defect.

<a id="review-1-6"></a>

### Claim 1.6 — Direct `tungstenite = "0.29"` dependency is unused and version-skewed vs the effective 0.24

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Unused direct 0.29.0 remains alongside the effective tokio-tungstenite re-export of 0.24.0.

Evidence: [crates/shamir-transport-ws/Cargo.toml:20](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/Cargo.toml#L20); [Cargo.lock:4256](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4256).

Grouping/duplicate: [security-crypto.md#1](security-crypto.md#review-1). This is not an additional independent defect.

<a id="review-1-7"></a>

### Claim 1.7 — `BrowserOriginPolicy::empty()` doc references a nonexistent `accept_no_origin` mode

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The documented exception has no implementation; None always fails with Missing.

Evidence: [crates/shamir-transport-ws/src/browser.rs:28](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/browser.rs#L28); [crates/shamir-transport-ws/src/browser.rs:99](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/browser.rs#L99).

Grouping/duplicate: [correctness-tdd.md#7](correctness-tdd.md#review-7). This is not an additional independent defect.

<a id="review-1-8"></a>

### Claim 1.8 — `BROWSER_CHANNEL_BINDING` is dead; the zeros invariant is encoded twice

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The unused constant and browser-loop zero literal duplicate the same currently correct bytes.

Evidence: [crates/shamir-transport-ws/src/tls_exporter.rs:25](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/tls_exporter.rs#L25); [crates/shamir-server/src/server/server_launcher.rs:1495](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_launcher.rs#L1495).

Grouping/duplicate: [style-claude-md.md#3](style-claude-md.md#review-3). This is not an additional independent defect.

<a id="review-1-9"></a>

### Claim 1.9 — Error-semantics / doc-accuracy warts

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Short Binary actual semantics and caller-selected pre-auth policy attribution both contradict their documentation.

Evidence: [crates/shamir-transport-ws/src/framing.rs:42](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/framing.rs#L42); [crates/shamir-transport-ws/src/framing.rs:151](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/framing.rs#L151); [crates/shamir-transport-ws/src/server.rs:27](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/server.rs#L27).

Grouping/duplicate: [correctness-tdd.md#9](correctness-tdd.md#review-9). This is not an additional independent defect.

<a id="review-2-1"></a>

### Claim 2.1 — Send framing ships only the allocating variant — production send path allocates + memcpys per message while recv is zero-alloc

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Send allocation/copy is real; zero-allocation receive applies only to fitting adapter scratch, not dependency assembly or fresh production request buffers.

Evidence: [crates/shamir-transport-ws/src/framing.rs:119](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/framing.rs#L119); [crates/shamir-server/src/connection/request_loop.rs:278](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L278).

Grouping/duplicate: [performance-hotpath.md#1](performance-hotpath.md#review-1). This is not an additional independent defect.

<a id="review-2-2"></a>

### Claim 2.2 — `accept_browser_ws` deep-clones the entire origin allowlist per accepted connection

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Transport and production spawn each copy Vec&lt;String&gt;; borrowing/sharing is supported by the pinned callback bounds.

Evidence: [crates/shamir-transport-ws/src/server.rs:118](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/server.rs#L118); [crates/shamir-server/src/server/server_launcher.rs:1472](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_launcher.rs#L1472); [Cargo.lock:4467](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4467).

Grouping/duplicate: [concurrency-lockfree.md#2](concurrency-lockfree.md#review-2). This is not an additional independent defect.

<a id="review-3-1"></a>

### Claim 3.1 — Phantom `tungstenite = "0.29"` dependency — two WS parsers compiled, the live one is the older

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Effective 0.24.0 remains unchanged by the direct-only historical bump. The unused 0.29.0 graph entry does not prove final-binary parser retention or a vulnerability.

Evidence: [crates/shamir-transport-ws/Cargo.toml:20](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/Cargo.toml#L20); [Cargo.lock:3781](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L3781); [Cargo.lock:4256](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4256).

Grouping/duplicate: [security-crypto.md#1](security-crypto.md#review-1). This is not an additional independent defect.

<a id="review-3-2"></a>

### Claim 3.2 — `accept_browser_ws` Origin enforcement has no live-wiring test coverage

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

Registered TS positive integration exercises production browser acceptance; negative security-control coverage is still missing.

Evidence: [crates/shamir-client-ts/src/__tests__/connect.test.ts:203](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-ts/src/__tests__/connect.test.ts#L203); [.github/workflows/ts-e2e-nightly.yml:85](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/.github/workflows/ts-e2e-nightly.yml#L85).

Grouping/duplicate: [correctness-tdd.md#1](correctness-tdd.md#review-1). This is not an additional independent defect.

<a id="review-3-3"></a>

### Claim 3.3 — Attacker-controlled `Origin` echoed into the HTTP 403 response body

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Raw denied-Origin reflection is present, but no exploitable rendering, credential exposure or log-control witness establishes low security severity.

Evidence: [crates/shamir-transport-ws/src/browser.rs:103](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/browser.rs#L103); [crates/shamir-transport-ws/src/server.rs:136](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/server.rs#L136).

Grouping/duplicate: [security-crypto.md#3](security-crypto.md#review-3). This is not an additional independent defect.

<a id="review-3-4"></a>

### Claim 3.4 — Unbounded control-frame loop in `ws_recv_into_stream` (ping-flood liveness); auto-pong write queue is uncapped

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Exact tungstenite/TLS source supports continued reads plus encoded Pong accumulation under blocked output during the unbounded proof read. Pending Pong is singular and reader flushing exists; connection caps do not bound per-peer storage.

Evidence: [crates/shamir-transport-ws/src/framing.rs:176](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/framing.rs#L176); [crates/shamir-server/src/connection/handshake.rs:291](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/handshake.rs#L291); [Cargo.lock:4223](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4223); [Cargo.lock:4467](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4467).

Grouping/duplicate: [security-crypto.md#4](security-crypto.md#review-4). This is not an additional independent defect.

<a id="review-3-5"></a>

### Claim 3.5 — `Option`-returning exporter API + public all-zeros constant invites silent zero-substitution on the native path

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Error erasure and native fallback remain; mode does not change. Failure of the real completed TLS exporter and an exploitable downgrade are not established.

Evidence: [crates/shamir-transport-tcp/src/tls.rs:81](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tls.rs#L81); [crates/shamir-server/src/server/server_launcher.rs:1391](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_launcher.rs#L1391); [crates/shamir-connect/src/server/handshake.rs:135](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/handshake.rs#L135).

Grouping/duplicate: [security-crypto.md#5](security-crypto.md#review-5). This is not an additional independent defect.

<a id="review-3-6"></a>

### Claim 3.6 — Doc misattributes the 4 KiB pre-auth cap to this crate's framing layer

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The server selects the pre-auth cap; the transport validates only the supplied number after message assembly.

Evidence: [crates/shamir-transport-ws/src/server.rs:27](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/server.rs#L27); [crates/shamir-server/src/connection/handshake.rs:717](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/handshake.rs#L717).

Grouping/duplicate: [performance-hotpath.md#4](performance-hotpath.md#review-4). This is not an additional independent defect.

<a id="review-3-7"></a>

### Claim 3.7 — `ws_send_sink` truncates the length prefix for payloads &gt;= 4 GiB

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Unchecked usize-to-u32 narrowing wraps at 2^32 bytes, subject to huge-buffer allocation and sink preconditions.

Evidence: [crates/shamir-transport-ws/src/framing.rs:118](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/framing.rs#L118).

Grouping/duplicate: [api-wire-protocol.md#3](api-wire-protocol.md#review-3). This is not an additional independent defect.

<a id="review-3-8"></a>

### Claim 3.8 — `BrowserOriginPolicy::allow` accepts malformed patterns that silently never match

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Malformed configuration can exclude intended valid Origins, but identical malformed headers can match; construction performs no validation.

Evidence: [crates/shamir-transport-ws/src/browser.rs:37](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/browser.rs#L37); [crates/shamir-transport-ws/src/browser.rs:75](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/browser.rs#L75).

Grouping/duplicate: [correctness-tdd.md#5](correctness-tdd.md#review-5). This is not an additional independent defect.

<a id="review-4-1"></a>

### Claim 4.1 — WSS send hot path: fresh heap alloc + full-payload copy per frame; TCP's prereserved zero-copy path is silently defeated

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The borrowed prereserved default strips the prefix, and WS allocates/copies it again. Structural optimization debt is confirmed; medium runtime impact is unmeasured.

Evidence: [crates/shamir-server/src/connection/request_loop.rs:198](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L198); [crates/shamir-server/src/framer.rs:123](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/framer.rs#L123); [crates/shamir-transport-ws/src/framing.rs:119](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/framing.rs#L119).

Grouping/duplicate: [performance-hotpath.md#1](performance-hotpath.md#review-1). This is not an additional independent defect.

<a id="review-4-2"></a>

### Claim 4.2 — Receive loop consumes unbounded consecutive control frames; auto-pong write queue is uncapped

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

The unbudgeted adapter and uncapped encoded output buffer permit the conditional proof-stage backpressure witness; exact dependency source confirms reader flushing rather than writer-only behavior.

Evidence: [crates/shamir-transport-ws/src/framing.rs:176](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/framing.rs#L176); [crates/shamir-transport-ws/src/server.rs:43](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/server.rs#L43); [Cargo.lock:4467](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4467).

Grouping/duplicate: [security-crypto.md#4](security-crypto.md#review-4). This is not an additional independent defect.

<a id="review-4-3"></a>

### Claim 4.3 — Dead direct dependency `tungstenite = "0.29"` compiles a second, unused copy of tungstenite

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The unused second dependency remains resolved; exact build cost and linked-code retention are not established.

Evidence: [crates/shamir-transport-ws/Cargo.toml:20](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/Cargo.toml#L20); [Cargo.lock:4487](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4487).

Grouping/duplicate: [security-crypto.md#1](security-crypto.md#review-1). This is not an additional independent defect.

<a id="review-4-4"></a>

### Claim 4.4 — Doc drift: `server_ws_config` claims a "4 KiB pre-auth logical check … enforced in `crate::framing::ws_recv_into`" — no such enforcement exists in this crate

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Intrinsic pre-auth attribution is inaccurate; the grouped short-frame actual documentation is a separate remaining issue.

Evidence: [crates/shamir-transport-ws/src/server.rs:27](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/server.rs#L27); [crates/shamir-transport-ws/src/framing.rs:151](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/framing.rs#L151).

Grouping/duplicate: [performance-hotpath.md#4](performance-hotpath.md#review-4). This is not an additional independent defect.

<a id="review-4-5"></a>

### Claim 4.5 — `accept_browser_ws` deep-clones the origin policy per connection accept

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The transport still clones immutable Vec&lt;String&gt; policy for its callback; runtime impact is unmeasured.

Evidence: [crates/shamir-transport-ws/src/server.rs:118](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/server.rs#L118).

Grouping/duplicate: [concurrency-lockfree.md#2](concurrency-lockfree.md#review-2). This is not an additional independent defect.

<a id="review-5-1"></a>

### Claim 5.1 — Spec-mandated WebSocket subprotocol negotiation is unimplemented

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

No subprotocol is selected. Browser clients offering shamir-v1 must reject the absent acknowledgement under WHATWG §2.2. First-party no-offer behavior and full-auth V1 checks do not fulfill the specified handshake.

Evidence: [docs/guide-docs/client-server-protocol-spec/TRANSPORT_WS.md:18](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/docs/guide-docs/client-server-protocol-spec/TRANSPORT_WS.md#L18); [crates/shamir-transport-ws/src/server.rs:88](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/server.rs#L88); [crates/shamir-client-ts/src/platform/browser.ts:118](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-ts/src/platform/browser.ts#L118).

Grouping/duplicate: [api-wire-protocol.md#1](api-wire-protocol.md#review-1). This is not an additional independent defect.

<a id="review-5-2"></a>

### Claim 5.2 — Endpoint paths hardcoded as string literals; no shared constants; incompatible with the server's configurable `path`

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

Accepted slash-prefixed alternatives never reach acceptors and receive 404. Requests to fixed protocol paths remain valid.

Evidence: [crates/shamir-server/src/config.rs:806](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/config.rs#L806); [crates/shamir-server/src/server/server_launcher.rs:869](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_launcher.rs#L869); [crates/shamir-transport-ws/src/server.rs:124](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/server.rs#L124).

Grouping/duplicate: [api-wire-protocol.md#2](api-wire-protocol.md#review-2). This is not an additional independent defect.

<a id="review-5-3"></a>

### Claim 5.3 — Send path has no frame-size cap and truncates the length prefix at `u32::MAX` — diverges from the TCP sibling

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

No local WS outbound cap exists; the default server result ceiling exceeds 16 MiB. Narrowing wraps above u32::MAX, not at it; sink success is conditional.

Evidence: [crates/shamir-transport-ws/src/framing.rs:118](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/framing.rs#L118); [crates/shamir-server/src/config.rs:418](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/config.rs#L418); [crates/shamir-transport-tcp/src/framing.rs:153](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L153).

Grouping/duplicate: [api-wire-protocol.md#3](api-wire-protocol.md#review-3). This is not an additional independent defect.

<a id="review-5-4"></a>

### Claim 5.4 — Unused, version-mismatched direct dependency `tungstenite = "0.29"` while the public API is pinned to tungstenite 0.24

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Public Error/Message types remain effective 0.24.0 despite direct 0.29.0. These are incompatible type identities.

Evidence: [crates/shamir-transport-ws/src/framing.rs:33](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/framing.rs#L33); [Cargo.lock:4256](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4256); [Cargo.lock:3781](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L3781).

Grouping/duplicate: [security-crypto.md#1](security-crypto.md#review-1). This is not an additional independent defect.

<a id="review-5-5"></a>

### Claim 5.5 — Zero-length frame means "graceful close" on TCP but is a legal empty frame on WS — undocumented divergence in a claimed-identical wire format

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The distinction is real but shared byte layout does not imply identical transport-close semantics. Documentation suffices; rejecting zero is not TCP graceful-close alignment.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:107](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L107); [crates/shamir-transport-ws/src/framing.rs:171](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/framing.rs#L171); [crates/shamir-transport-ws/src/framing.rs:173](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/framing.rs#L173).

Grouping/duplicate: [api-wire-protocol.md#5](api-wire-protocol.md#review-5). This is not an additional independent defect.

<a id="review-5-6"></a>

### Claim 5.6 — `accept_browser_ws` — the Origin-enforcing handshake path — has zero integration tests; the framing length-mismatch invariant is also untested

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

The absolute browser clause is refuted by conditional TS integration; targeted browser rejection and malformed-frame oracles remain absent.

Evidence: [crates/shamir-client-ts/src/__tests__/connect.test.ts:203](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-ts/src/__tests__/connect.test.ts#L203); [crates/shamir-transport-ws/tests/framing_round_trip.rs:69](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/tests/framing_round_trip.rs#L69).

Grouping/duplicate: [correctness-tdd.md#2](correctness-tdd.md#review-2). This is not an additional independent defect.

<a id="review-5-7"></a>

### Claim 5.7 — Exporter-extraction ordering: this crate's doc contradicts its only production caller

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Launcher inaccessibility prose is false: get_ref supports post-upgrade extraction, as the integration source demonstrates. Both TLS-completed orders work.

Evidence: [crates/shamir-transport-ws/src/server.rs:7](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/server.rs#L7); [crates/shamir-server/src/server/server_launcher.rs:1388](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_launcher.rs#L1388); [crates/shamir-server/tests/mvp_ws_e2e.rs:195](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/tests/mvp_ws_e2e.rs#L195).

Grouping/duplicate: [api-wire-protocol.md#7](api-wire-protocol.md#review-7). This is not an additional independent defect.

<a id="review-5-8"></a>

### Claim 5.8 — `BROWSER_CHANNEL_BINDING` constant exported but consumers re-hardcode `[0u8; 32]`

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Browser duplication and native fallback are distinct remaining concerns. Substituting the named browser constant on native failure would preserve the fallback defect.

Evidence: [crates/shamir-server/src/server/server_launcher.rs:1391](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_launcher.rs#L1391); [crates/shamir-server/src/server/server_launcher.rs:1495](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_launcher.rs#L1495); [crates/shamir-transport-ws/src/tls_exporter.rs:25](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/tls_exporter.rs#L25).

Grouping/duplicate: [security-crypto.md#5](security-crypto.md#review-5). This is not an additional independent defect.

<a id="review-5-9"></a>

### Claim 5.9 — `accept_browser_ws` clones the origin policy on every connection

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Both deep clones remain; exact callback bounds permit borrowing and launcher policy can be shared.

Evidence: [crates/shamir-transport-ws/src/server.rs:118](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/server.rs#L118); [crates/shamir-server/src/server/server_launcher.rs:1472](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_launcher.rs#L1472).

Grouping/duplicate: [concurrency-lockfree.md#2](concurrency-lockfree.md#review-2). This is not an additional independent defect.

<a id="review-5-10"></a>

### Claim 5.10 — Origin matching is case-sensitive; wildcard detection is a whole-pattern substring search

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Raw matching and unanchored wildcard detection remain. Literal exact policy is documented; RFC origin serialization does not mandate this API normalize arbitrary configuration.

Evidence: [crates/shamir-transport-ws/src/browser.rs:16](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/browser.rs#L16); [crates/shamir-transport-ws/src/browser.rs:59](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/browser.rs#L59); [crates/shamir-transport-ws/src/browser.rs:75](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/browser.rs#L75).

Grouping/duplicate: [correctness-tdd.md#5](correctness-tdd.md#review-5). This is not an additional independent defect.

<a id="review-5-11"></a>

### Claim 5.11 — `is_loopback` re-implements `IpAddr::is_loopback`

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Equivalent private V4/V6 dispatch remains; simplification is optional maintenance.

Evidence: [crates/shamir-transport-ws/src/listener.rs:47](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/listener.rs#L47).

Grouping/duplicate: [style-claude-md.md#6](style-claude-md.md#review-6). This is not an additional independent defect.

<a id="review-5-12"></a>

### Claim 5.12 — Unused dev-dependencies: `hex`, `serde`, `serde_bytes`, `rmp-serde` (plus the direct `tungstenite`, see 3.1)

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The unused entries persist; opaque-byte framing does not need MessagePack deserialization to be valid.

Evidence: [crates/shamir-transport-ws/Cargo.toml:20](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/Cargo.toml#L20); [crates/shamir-transport-ws/Cargo.toml:32](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/Cargo.toml#L32).

Grouping/duplicate: [style-claude-md.md#5](style-claude-md.md#review-5). This is not an additional independent defect.

<a id="review-6-1"></a>

### Claim 6.1 — `WsAcceptError::OriginRejected` and `WrongPath` are dead variants — all accept rejections surface as `Handshake`

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Both acceptors return nested Http handshake errors rather than typed policy variants. Status classification requires no response-body parsing.

Evidence: [crates/shamir-transport-ws/src/server.rs:55](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/server.rs#L55); [crates/shamir-transport-ws/src/server.rs:99](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/server.rs#L99); [crates/shamir-transport-ws/src/server.rs:144](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/server.rs#L144).

Grouping/duplicate: [error-handling-lifecycle.md#1](error-handling-lifecycle.md#review-1). This is not an additional independent defect.

<a id="review-6-2"></a>

### Claim 6.2 — Error paths of framing and accept have no variant-asserting tests

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Positive native/TS integration does not assert policy-rejection variants or malformed-message outcomes. The negative adapter test accepts any error.

Evidence: [crates/shamir-transport-ws/tests/framing_round_trip.rs:69](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/tests/framing_round_trip.rs#L69); [crates/shamir-transport-ws/src/tests/server_tests.rs:7](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/tests/server_tests.rs#L7).

Grouping/duplicate: [correctness-tdd.md#2](correctness-tdd.md#review-2). This is not an additional independent defect.

<a id="review-6-3"></a>

### Claim 6.3 — Send path lacks the TCP sibling's size guard; `payload.len() as u32` truncates silently

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The sender still narrows length and allocates without TCP's local ceiling.

Evidence: [crates/shamir-transport-ws/src/framing.rs:118](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/framing.rs#L118); [crates/shamir-transport-tcp/src/framing.rs:153](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L153).

Grouping/duplicate: [api-wire-protocol.md#3](api-wire-protocol.md#review-3). This is not an additional independent defect.

<a id="review-6-4"></a>

### Claim 6.4 — Exporter extraction failure is indistinguishable from unavailability (`Option` discards the cause)

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The delegated helper erases its error using ok(); the wrapper cannot recover it. Native fallback remains a separate caller policy concern.

Evidence: [crates/shamir-transport-ws/src/tls_exporter.rs:20](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/tls_exporter.rs#L20); [crates/shamir-transport-tcp/src/tls.rs:81](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tls.rs#L81).

Grouping/duplicate: [security-crypto.md#5](security-crypto.md#review-5). This is not an additional independent defect.

<a id="review-6-5"></a>

### Claim 6.5 — Framing error leaves the caller's scratch buffer holding the previous frame

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

Preserving initialized prior bytes on Err violates no documented postcondition. Current callers check errors before decoding. An empty-on-error promise would be an optional API change, not a proven replay fix.

Evidence: [crates/shamir-transport-ws/src/framing.rs:169](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/framing.rs#L169); [crates/shamir-server/src/connection/handshake.rs:719](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/handshake.rs#L719); [crates/shamir-server/src/connection/request_loop.rs:280](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L280).

Grouping/duplicate: [error-handling-lifecycle.md#5](error-handling-lifecycle.md#review-5). This is not an additional independent defect.

<a id="review-6-6"></a>

### Claim 6.6 — Doc attributes the 4 KiB pre-auth check to `ws_recv_into`, which enforces nothing by itself

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

It enforces the caller's cap, but authentication phase and 4 KiB selection live outside the adapter.

Evidence: [crates/shamir-transport-ws/src/server.rs:27](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/server.rs#L27); [crates/shamir-transport-ws/src/framing.rs:163](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/framing.rs#L163).

Grouping/duplicate: [performance-hotpath.md#4](performance-hotpath.md#review-4). This is not an additional independent defect.

<a id="review-6-7"></a>

### Claim 6.7 — `Origin` header containing obs-text bytes is reported as `Missing` rather than rejected-as-present

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

A present HeaderValue failing visible-ASCII conversion becomes None, so Missing is reported while the upgrade still rejects.

Evidence: [crates/shamir-transport-ws/src/server.rs:132](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/server.rs#L132); [crates/shamir-transport-ws/src/browser.rs:99](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/browser.rs#L99); [Cargo.lock:1672](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L1672).

Grouping/duplicate: [error-handling-lifecycle.md#7](error-handling-lifecycle.md#review-7). This is not an additional independent defect.

<a id="review-7-1"></a>

### Claim 7.1 — Mid-file `use` statement violates the "imports at the top" rule

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The integration import remains below four functions; this violates a placement convention without declaration-order runtime semantics.

Evidence: [crates/shamir-transport-ws/tests/framing_round_trip.rs:98](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/tests/framing_round_trip.rs#L98); [CLAUDE.md:610](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L610).

Grouping/duplicate: [style-claude-md.md#1](style-claude-md.md#review-1). This is not an additional independent defect.

<a id="review-7-2"></a>

### Claim 7.2 — `lib.rs` re-export set incomplete vs. module public APIs (and vs. sibling transport crate) — partially corrected on spot-check

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

ws_recv was root-exported from the initial commit; MAX_WS_FRAME_SIZE remains publicly nameable. No all-items-at-root rule exists. The surviving suggestion is optional consistency, so the current open summary verdict contradicts its own qualification and the thematic refutation.

Evidence: [crates/shamir-transport-ws/src/lib.rs:24](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/lib.rs#L24); [crates/shamir-transport-ws/src/lib.rs:31](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/lib.rs#L31); [crates/shamir-transport-ws/src/framing.rs:26](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/framing.rs#L26).

Grouping/duplicate: [style-claude-md.md#2](style-claude-md.md#review-2). This is not an additional independent defect.

<a id="review-7-3"></a>

### Claim 7.3 — `BROWSER_CHANNEL_BINDING` is dead public API; the zero placeholder exists only in prose elsewhere

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Repository non-use remains, but real production literals refute the prose-only facet. This is maintenance duplication, not incorrect browser binding.

Evidence: [crates/shamir-transport-ws/src/tls_exporter.rs:25](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/tls_exporter.rs#L25); [crates/shamir-server/src/server/server_launcher.rs:1495](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_launcher.rs#L1495).

Grouping/duplicate: [style-claude-md.md#3](style-claude-md.md#review-3). This is not an additional independent defect.

<a id="review-7-4"></a>

### Claim 7.4 — Unused direct dependency `tungstenite = "0.29"` (version-mismatched with tokio-tungstenite)

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The unused direct package is distinct from the effective 0.24.0 public and parsing implementation.

Evidence: [crates/shamir-transport-ws/Cargo.toml:20](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/Cargo.toml#L20); [Cargo.lock:4256](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4256).

Grouping/duplicate: [security-crypto.md#1](security-crypto.md#review-1). This is not an additional independent defect.

<a id="review-7-5"></a>

### Claim 7.5 — Unused `[dev-dependencies]`: `hex`, `serde`, `serde_bytes`, `rmp-serde`

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

All four remain unused by registered crate source/tests. This is dependency hygiene only.

Evidence: [crates/shamir-transport-ws/Cargo.toml:32](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/Cargo.toml#L32).

Grouping/duplicate: [style-claude-md.md#5](style-claude-md.md#review-5). This is not an additional independent defect.

<a id="review-7-6"></a>

### Claim 7.6 — Redundant `is_loopback` helper duplicates `IpAddr::is_loopback`

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The equivalent helper remains. Existing predicate tests do not imply a behavior defect requiring repair.

Evidence: [crates/shamir-transport-ws/src/listener.rs:47](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/listener.rs#L47); [crates/shamir-transport-ws/src/tests/listener_tests.rs:62](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/tests/listener_tests.rs#L62).

Grouping/duplicate: [style-claude-md.md#6](style-claude-md.md#review-6). This is not an additional independent defect.

## Revalidated plan decisions

| Plan decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 16 | 16 | 0 | 0 | 0 | 0 | 0 |

Historical P0/P1/P2 numbering is an identifier, not a current release mandate. The reasons below include completion status, safety qualifications and discriminating acceptance requirements.

<a id="plan-p0-1"></a>

### Plan P0.1 — P0.1

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Configured paths remain ignored. Either reject unsupported fixed paths during boot or carry validated expected paths into matching-profile acceptors; constants alone are insufficient. Do not permit path changes to alter binding-mode policy.

Evidence: [crates/shamir-server/src/config.rs:806](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/config.rs#L806); [crates/shamir-server/src/server/server_launcher.rs:869](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_launcher.rs#L869); [crates/shamir-transport-ws/src/server.rs:124](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/server.rs#L124).

<a id="plan-p0-2"></a>

### Plan P0.2 — P0.2

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Negotiation and coordinated client requests remain absent. Select one supported offered token and return it; missing/incompatible offers need an explicit compatibility policy. An unconditional enforcement change would break first-party clients currently offering none.

Evidence: [crates/shamir-transport-ws/src/server.rs:88](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/server.rs#L88); [crates/shamir-client-ts/src/platform/browser.ts:118](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-ts/src/platform/browser.ts#L118); [docs/guide-docs/client-server-protocol-spec/TRANSPORT_WS.md:18](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/docs/guide-docs/client-server-protocol-spec/TRANSPORT_WS.md#L18).

<a id="plan-p0-3"></a>

### Plan P0.3 — P0.3

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Positive TS browser coverage already exists; rejection and exact malformed-frame oracles remain missing. Require missing Origin=400, denied Origin=403, wrong path=404, and precise framing variants. Removing a rejection branch must fail the negative oracle.

Evidence: [crates/shamir-client-ts/src/__tests__/connect.test.ts:203](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-ts/src/__tests__/connect.test.ts#L203); [crates/shamir-transport-ws/tests/framing_round_trip.rs:69](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/tests/framing_round_trip.rs#L69); [docs/guide-docs/client-server-protocol-spec/TRANSPORT_WS.md:92](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/docs/guide-docs/client-server-protocol-spec/TRANSPORT_WS.md#L92).

<a id="plan-p0-4"></a>

### Plan P0.4 — P0.4

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Unused direct tungstenite/rustls/tokio-rustls entries remain. Removal is source-supported hygiene without a version bump, but must preserve transitive feature requirements and public effective types; it does not update the live parser or prove a CVE fix.

Evidence: [crates/shamir-transport-ws/Cargo.toml:20](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/Cargo.toml#L20); [crates/shamir-transport-ws/Cargo.toml:23](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/Cargo.toml#L23); [Cargo.lock:4256](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4256).

<a id="plan-p1-5"></a>

### Plan P1.5 — P1.5

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

No outbound guard exists. Reject above the logical ceiling before allocation and narrowing, preserve TooLarge details, and separately provide codec headroom for the inner prefix. Checked u32 conversion alone does not enforce 16 MiB.

Evidence: [crates/shamir-transport-ws/src/framing.rs:118](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/framing.rs#L118); [crates/shamir-transport-ws/src/server.rs:46](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/server.rs#L46); [crates/shamir-transport-tcp/src/framing.rs:153](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L153).

<a id="plan-p1-6"></a>

### Plan P1.6 — P1.6

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Typed rejection variants remain unproduced. Capture structured callback rejection/path locally and map after the HTTP response is sent, or deliberately document/change taxonomy. Status-only/body parsing cannot preserve original details, especially after P2.14.

Evidence: [crates/shamir-transport-ws/src/server.rs:55](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/server.rs#L55); [crates/shamir-transport-ws/src/server.rs:99](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/server.rs#L99); [crates/shamir-transport-ws/src/server.rs:136](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/server.rs#L136); [Cargo.lock:4467](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4467).

<a id="plan-p1-7"></a>

### Plan P1.7 — P1.7

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

No owned send seam exists. An owned prefixed Vec requires coordinated writer/caller changes, validates exactly one prefix, and must keep ByteBudgetGuard until write completion/error. Cloning borrowed scratch or prepending via splice is not zero-copy/zero-allocation proof.

Evidence: [crates/shamir-transport-ws/src/framing.rs:114](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/framing.rs#L114); [crates/shamir-server/src/framer.rs:110](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/framer.rs#L110); [crates/shamir-server/src/connection/request_loop.rs:198](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L198).

<a id="plan-p1-8"></a>

### Plan P1.8 — P1.8

Status: `confirmed-open`.

Prior-cycle decision: `unverified`.

Exact 0.24.0 source resolves the premise: finite max_write_buffer_size is already supported and absent here; encoded output can grow under backpressure. Add a terminal absolute proof-stage deadline and bounded cleanup. A small consecutive-control quota alone can reject legitimate long-idle heartbeats and does not cover stalled partial messages; an upgrade or resettable idle timer alone is insufficient.

Evidence: [crates/shamir-transport-ws/src/server.rs:43](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/server.rs#L43); [crates/shamir-transport-ws/src/framing.rs:176](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/framing.rs#L176); [crates/shamir-server/src/connection/handshake.rs:291](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/handshake.rs#L291); [Cargo.lock:4467](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4467); [Cargo.lock:4223](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4223).

<a id="plan-p1-9"></a>

### Plan P1.9 — P1.9

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Error erasure and native fallback remain. Preserving the underlying error and aborting native extraction failure is semantically safe; warning and silently changing modes is not. Use the placeholder only on the browser/explicit no-export branch, not both launcher sites.

Evidence: [crates/shamir-transport-tcp/src/tls.rs:81](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tls.rs#L81); [crates/shamir-server/src/server/server_launcher.rs:1391](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_launcher.rs#L1391); [crates/shamir-server/src/server/server_launcher.rs:1495](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_launcher.rs#L1495); [crates/shamir-connect/src/server/handshake.rs:135](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/handshake.rs#L135).

<a id="plan-p1-10"></a>

### Plan P1.10 — P1.10

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Ordering prose remains inconsistent. Document completed-TLS extraction either before upgrade or through get_ref afterward; imposing mandatory before-upgrade ordering would replace one false assurance with another.

Evidence: [crates/shamir-transport-ws/src/server.rs:7](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/server.rs#L7); [crates/shamir-server/src/server/server_launcher.rs:1388](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_launcher.rs#L1388); [crates/shamir-server/tests/mvp_ws_e2e.rs:195](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/tests/mvp_ws_e2e.rs#L195).

<a id="plan-p1-11"></a>

### Plan P1.11 — P1.11

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

The semantic distinction remains undocumented. Clarifying WS Close versus TCP zero-length close is sufficient. Rejecting zero as a protocol error is not identical to TCP PeerClose and needs a deliberate compatibility decision.

Evidence: [crates/shamir-transport-ws/src/framing.rs:3](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/framing.rs#L3); [crates/shamir-transport-ws/src/framing.rs:171](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/framing.rs#L171); [crates/shamir-transport-tcp/src/framing.rs:107](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L107).

<a id="plan-p2-12"></a>

### Plan P2.12 — P2.12

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Pattern validation remains absent. Prefer release-enforced fallible validation with explicit exact/wildcard/port/opaque-origin policy. debug_assert is not production validation; blindly lowercasing arbitrary strings or banning colons would mishandle valid structured forms.

Evidence: [crates/shamir-transport-ws/src/browser.rs:37](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/browser.rs#L37); [crates/shamir-transport-ws/src/browser.rs:59](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/browser.rs#L59); [crates/shamir-server/src/server/server_launcher.rs:1014](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_launcher.rs#L1014).

<a id="plan-p2-13"></a>

### Plan P2.13 — P2.13

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Both clones persist. Exact callback bounds support a borrowed policy; sharing launcher policy with Arc avoids the outer clone. No explicit lifetime annotation or Arc-specific public overload is required merely to remove the inner copy.

Evidence: [crates/shamir-transport-ws/src/server.rs:118](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/server.rs#L118); [crates/shamir-server/src/server/server_launcher.rs:1472](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_launcher.rs#L1472); [Cargo.lock:4245](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4245); [Cargo.lock:4467](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4467).

<a id="plan-p2-14"></a>

### Plan P2.14 — P2.14

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Reflection remains. A static rejection body is optional hardening; preserve any desired typed/internal diagnostic separately and coordinate with P1.6. Structured logging does not justify logging unbounded raw data or prove an XSS fix.

Evidence: [crates/shamir-transport-ws/src/server.rs:136](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/server.rs#L136); [crates/shamir-server/src/server/server_launcher.rs:1502](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_launcher.rs#L1502).

<a id="plan-p2-15"></a>

### Plan P2.15 — P2.15

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Incorrect pre-auth attribution, short-message field documentation, nonexistent bypass and invalid-header classification remain. Error-buffer documentation and root constant export are optional; ws_recv needs no repair. Also correct phase-cap, fragmentation and post-upgrade-access assurances.

Evidence: [crates/shamir-transport-ws/src/server.rs:27](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/server.rs#L27); [crates/shamir-transport-ws/src/browser.rs:28](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/browser.rs#L28); [crates/shamir-transport-ws/src/framing.rs:151](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/framing.rs#L151); [crates/shamir-transport-ws/src/server.rs:132](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/server.rs#L132); [crates/shamir-transport-ws/src/lib.rs:31](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/lib.rs#L31).

<a id="plan-p2-16"></a>

### Plan P2.16 — P2.16

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Import placement and unused dev entries remain, with an optional equivalent loopback simplification. Keep any future cleanup surgical; adding MessagePack parsing solely to justify dependencies or a repository-wide format sweep is not necessary.

Evidence: [crates/shamir-transport-ws/tests/framing_round_trip.rs:98](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/tests/framing_round_trip.rs#L98); [crates/shamir-transport-ws/Cargo.toml:32](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/Cargo.toml#L32); [crates/shamir-transport-ws/src/listener.rs:47](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/listener.rs#L47).

## Additional observations

| Observation decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 4 | 4 | 0 | 0 | 0 | 0 | 0 |

Existing observation IDs remain stable. New cycle-2 rows follow them; cross-module duplicates and extra triggers are grouped explicitly. None is an implemented fix.

<a id="observation-new-1"></a>

### Observation NEW.1 — Accepted WS codec ceilings exclude the final four bytes of the documented logical payload range

Status: `confirmed-open`. Current risk: `medium`.

Additional observation in this independent cycle; it may overlap an existing root.

Let M=16 MiB. A valid Binary body is four prefix bytes plus an M-byte payload. server_ws_config limits both WS frame payload and assembled message to M, so payload sizes M-3 through M are rejected before the logical validator despite meeting the TCP-equivalent payload limit. Fragmenting does not evade the assembled-message cap. Existing small and M+1 raw-message tests miss this. Oracle: valid prefixed M-4, M-3 and M payloads, plus M+1 rejection, through the actual accept path. Exact tungstenite 0.24.0 read_frame/IncompleteMessage::extend count WS payload/message bytes.

Evidence: [crates/shamir-transport-ws/src/server.rs:46](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/server.rs#L46); [crates/shamir-transport-ws/src/framing.rs:119](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/framing.rs#L119); [crates/shamir-connect/src/common/types.rs:111](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/common/types.rs#L111); [docs/guide-docs/client-server-protocol-spec/TRANSPORT_WS.md:78](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/docs/guide-docs/client-server-protocol-spec/TRANSPORT_WS.md#L78); [docs/guide-docs/client-server-protocol-spec/TRANSPORT_TCP.md:30](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/docs/guide-docs/client-server-protocol-spec/TRANSPORT_TCP.md#L30); [Cargo.lock:4467](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4467).

<a id="observation-new-2"></a>

### Observation NEW.2 — Native WS acceptance ignores supplied Origin despite the endpoint policy

Status: `confirmed-open`. Current risk: `medium`.

Additional observation in this independent cycle; it may overlap an existing root.

A syntactically valid upgrade to /shamir/v1 containing Origin:https://evil.example passes the native callback because only the path is checked. TRANSPORT_WS §9.2 requires a supplied Origin to match allowed_origins. The native accept API cannot receive that policy, and production supplies none. This is a pre-authentication policy omission, not a demonstrated exporter-authentication or cookie bypass. Oracle: absent native Origin succeeds, supplied allowed Origin succeeds, supplied denied Origin rejects before 101.

Evidence: [crates/shamir-transport-ws/src/server.rs:78](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/server.rs#L78); [crates/shamir-transport-ws/src/server.rs:88](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/server.rs#L88); [crates/shamir-server/src/server/server_launcher.rs:1394](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_launcher.rs#L1394); [docs/guide-docs/client-server-protocol-spec/TRANSPORT_WS.md:96](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/docs/guide-docs/client-server-protocol-spec/TRANSPORT_WS.md#L96).

<a id="observation-new-3"></a>

### Observation NEW.3 — Transport rejection discards the specified WebSocket close-code semantics

Status: `confirmed-open`. Current risk: `medium`.

Additional observation in this independent cycle; it may overlap an existing root.

A peer sends Text: ws_recv_into_stream returns NonBinaryMessage, map_ws_err produces generic Decode, and the request reader breaks. The writer subsequently closes with None rather than code 1003. Malformed-length and oversized-message paths likewise lack the specified 1002/1009 mapping; codec Capacity errors are stringified as Io. Rejecting data is supported, but interoperable close reasons are not. Oracle: inspect the peer's received Close frame and its exact code after each controlled invalid input, not merely any Err or EOF.

Evidence: [crates/shamir-transport-ws/src/framing.rs:177](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/framing.rs#L177); [crates/shamir-server/src/framer.rs:360](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/framer.rs#L360); [crates/shamir-server/src/framer.rs:430](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/framer.rs#L430); [crates/shamir-server/src/connection/request_loop.rs:280](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L280); [docs/guide-docs/client-server-protocol-spec/TRANSPORT_WS.md:20](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/docs/guide-docs/client-server-protocol-spec/TRANSPORT_WS.md#L20); [docs/guide-docs/client-server-protocol-spec/TRANSPORT_WS.md:57](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/docs/guide-docs/client-server-protocol-spec/TRANSPORT_WS.md#L57).

<a id="observation-new-4"></a>

### Observation NEW.4 — Specified server heartbeat and Pong deadline have no production implementation

Status: `confirmed-open`. Current risk: `medium`.

Additional observation in this independent cycle; it may overlap an existing root.

TRANSPORT_WS §7 requires server-initiated Ping every 30 seconds and a 10-second Pong response deadline. The crate only skips received controls; the server writer waits for application messages and contains no Ping generator or correlated Pong tracking. The reader's 600-second connection-idle timer is a different mechanism. A stalled idle peer is therefore not handled under the promised heartbeat contract. Oracle: virtual-time production-seam tests observe Ping initiation, matched Pong acceptance and terminal timeout. Correct the spec first so timeout is not implemented by transmitting reserved 1006.

Evidence: [docs/guide-docs/client-server-protocol-spec/TRANSPORT_WS.md:70](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/docs/guide-docs/client-server-protocol-spec/TRANSPORT_WS.md#L70); [crates/shamir-transport-ws/src/framing.rs:176](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/framing.rs#L176); [crates/shamir-server/src/connection/request_loop.rs:175](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L175); [crates/shamir-server/src/connection/request_loop.rs:297](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L297); [crates/shamir-tunables/src/lib.rs:55](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tunables/src/lib.rs#L55).

## Evidence and recipe corrections

- The summary's current 47-open/2-refuted ledger should not be retained unchanged: Claim 7.2 is refuted and Claim 6.5 is not-applicable. These are not source fixes.
- P1.8 unverified becomes confirmed-open after exact locked parser and TLS-stack inspection. The summary's overview, corrections and follow-up order still contain superseded queue-verification qualifications.
- Claim 5.1 medium becomes high for spec-conformant browser clients: mandatory failure is normatively established by WHATWG, without asserting an executed browser result. The statement that only configured-path failure is high-impact is therefore too broad.
- Claim 4.1 medium becomes low: allocation/copy is proven but medium runtime impact is not. Claim 3.3 and zero-length clarification are nits, not established security/runtime defects.
- The original source has no post-report remediation in the checked transport paths. The initial ws_recv export and positive browser integration are counter-evidence, not later fixes.
- server_ws_config's 16 MiB value bounds message/frame payloads, not aggregate memory or one-frame-per-message structure. tokio-tungstenite 0.24.0 exposes get_config but not the synchronous WebSocket::set_config API; the proposed phase-adjustment recipe needs a supported integration seam rather than a direct call to that unavailable wrapper method.
- lib.rs says the anti-downgrade matrix prevents upgrading binding strength; the documented policy instead concerns downgrade rejection and configurable stronger-tier resumption. Its prose must not reverse the direction.
- Wildcard origin validation is not authentication. Optional normalization must operate on structured scheme/host/port policy, not presume every raw accepted string is a canonical browser origin.
- The documentation's heartbeat instruction must not send reserved code 1006 on wire. Close-code handling should preserve the specified supported failure categories and use bounded teardown.
- No root export, redundant helper, preserved-error-buffer or unused-constant issue warrants an unconditional release blocker.

## Module scope and limitations

Coverage: 8 assigned documents, 98 current claim rows, 16 plan rows, 0 pre-existing observation rows; 4 added observation rows in this cycle. Counts are calculated from the accepted rows.

Assigned documents: [api-wire-protocol.md](api-wire-protocol.md); [concurrency-lockfree.md](concurrency-lockfree.md); [correctness-tdd.md](correctness-tdd.md); [error-handling-lifecycle.md](error-handling-lifecycle.md); [performance-hotpath.md](performance-hotpath.md); [security-crypto.md](security-crypto.md); [style-claude-md.md](style-claude-md.md); [SUMMARY.md](SUMMARY.md).

- Read-only inspection only: no files written, programs reproduced, builds, tests, benchmarks, dependency operations, worktrees or child agents.
- All eight assigned documents, including current ledgers, parent refinements, corrections and collapsed historical bodies, were inspected. Historical repetitions were not counted again.
- HEAD matched the required base at both checks. Cargo.lock became modified during inspection, with external rustls/aws-lc/webpki upgrades. The frozen dependency pins were reconfirmed using git show at the required base; reviewed source and assigned reports had no working-tree differences. Results concern the frozen base, not the subsequently modified lockfile.
- Effective tokio-tungstenite 0.24.0 and tungstenite 0.24.0 cached archives were read to stdout and their SHA-256 hashes matched the frozen lockfile. Exact tokio-rustls 0.26.4, rustls 0.23.37, http 1.4.0 and futures-util 0.3.32 source was also inspected for relevant mechanisms.
- Test registration and discriminating assertions were assessed, not execution. TS positive integration requires a usable server binary and certificate-generation setup; it is not a real-browser execution certificate.
- No measured CPU, latency, throughput, RSS, executable-size or exploit threshold is asserted. This is bounded claim revalidation, not a complete TLS, authentication, resumption or supply-chain audit.

## Guarantee checks

- **Repository error handling, ownership and test-layout conventions** — `supported`. Implementation uses typed thiserror results, has no crate-owned unsafe, locks, atomics, production tasks or files, and correctly registers separated unit tests. Failed owned-stream acceptance releases ownership. These facts do not establish dependency-wide lock freedom or complete negative coverage. Reference: AGENTS.md; CLAUDE.md:610; crates/shamir-transport-ws/src/lib.rs:37; crates/shamir-transport-ws/src/tests/mod.rs:1.
- **WSS production configuration permits TLS 1.3 only and rejects early data** — `supported`. Production uses the TCP helper selecting only TLS13. Exact rustls 0.23.37 published src/server/builder.rs initializes max_early_data_size to zero. Generic WS acceptors themselves require only AsyncRead/AsyncWrite; TLS is a documented caller precondition, not a type-enforced property. Published source: https://docs.rs/crate/rustls/0.23.37/source/src/server/builder.rs. Reference: docs/guide-docs/client-server-protocol-spec/TRANSPORT_WS.md:24; crates/shamir-transport-tcp/src/tls.rs:57; crates/shamir-server/src/tls.rs:115; Cargo.lock:3048.
- **Both endpoints negotiate shamir-v1; incompatible offers receive HTTP 400** — `diverges`. Neither callback inspects or selects a subprotocol. A real browser offering shamir-v1 must fail the unacknowledged handshake under [WHATWG WebSockets §2.2](https://websockets.spec.whatwg.org/#opening-handshake). This consequence is now established normatively; it was previously left unverified. Reference: docs/guide-docs/client-server-protocol-spec/TRANSPORT_WS.md:18; crates/shamir-transport-ws/src/server.rs:88; crates/shamir-transport-ws/src/server.rs:123.
- **Browser Origin policy rejects before upgrade; missing Origin is 400 and mismatching Origin is 403** — `diverges`. Validation occurs before 101 and rejects both cases, but the callback uses 403 for both Missing and NotAllowed. Present non-ASCII header values also become Missing. Exact http 1.4.0 HeaderValue::to_str rejects non-visible-ASCII bytes, not merely malformed UTF-8: https://docs.rs/crate/http/1.4.0/source/src/header/value.rs. Reference: docs/guide-docs/client-server-protocol-spec/TRANSPORT_WS.md:89; crates/shamir-transport-ws/src/server.rs:129; crates/shamir-transport-ws/src/browser.rs:99.
- **Native endpoint validates Origin when one is supplied** — `diverges`. The native callback checks only the path and has no origin-policy argument. A valid upgrade with an arbitrary supplied Origin receives 101. This is a project-policy omission, not proof that a browser can satisfy native exporter authentication. Reference: docs/guide-docs/client-server-protocol-spec/TRANSPORT_WS.md:96; crates/shamir-transport-ws/src/server.rs:88.
- **Configured WS paths are honored or rejected during configuration validation** — `diverges`. Slash-prefixed alternatives pass validation but are not passed to either accept loop. Requests using /db-ws receive 404; fixed protocol paths remain usable. Reference: crates/shamir-server/src/config.rs:801; crates/shamir-server/src/server/server_launcher.rs:800; crates/shamir-transport-ws/src/server.rs:90.
- **Frame length is u32 big-endian, excludes the prefix, and equals the binary message body length** — `supported`. For representable sender lengths, emission and validation implement this layout. Short binaries and unequal lengths are rejected before scratch mutation. Registered symmetric roundtrips do not independently pin golden wire bytes or malformed-input variants. Reference: crates/shamir-transport-ws/src/framing.rs:118; crates/shamir-transport-ws/src/framing.rs:148; docs/guide-docs/client-server-protocol-spec/TRANSPORT_TCP.md:23.
- **The data payload ceiling is 16 MiB, identical to TCP** — `diverges`. Outbound WS has no local ceiling. Inbound codec ceilings count the four-byte inner prefix, so the configured server rejects valid logical payloads in the final four bytes of the advertised range. Reference: docs/guide-docs/client-server-protocol-spec/TRANSPORT_WS.md:78; crates/shamir-connect/src/common/types.rs:111; crates/shamir-transport-ws/src/server.rs:46.
- **The logical pre-authentication ceiling also prevents pre-authentication large-message buffering** — `diverges`. The server supplies the 4 KiB logical cap only after tungstenite 0.24.0 assembles a message. The accepted codec permits substantially larger pre-authentication messages. Its message/frame limits are not total per-connection memory ceilings, and equal limits do not prohibit fragmentation. Reference: crates/shamir-transport-ws/src/server.rs:27; crates/shamir-server/src/connection/handshake.rs:717; Cargo.lock:4467.
- **All unauthenticated reads have finite deadlines** — `diverges`. TLS, upgrade and AuthInit are bounded, but the challenge's subsequent client_proof read is awaited without an enclosing deadline. A peer can reach this stage without valid credentials and then withhold a proof. Reference: crates/shamir-server/src/server/server_launcher.rs:1370; crates/shamir-server/src/server/server_launcher.rs:1392; crates/shamir-server/src/connection/handshake.rs:291.
- **Automatic Pong handling is bounded and does not require reader-side flushing** — `diverges`. Exact tungstenite 0.24.0 has one replaceable pending Pong and flushes during reads, refuting writer-only flushing. However encoded Pong frames append to an effectively uncapped output Vec before blocked writes; read ignores WouldBlock. Exact tokio-rustls 0.26.4 and rustls 0.23.37 permit post-handshake reads despite pending output, supporting the deployed conditional witness. Sources: https://docs.rs/crate/tungstenite/0.24.0/source/src/protocol/mod.rs; https://docs.rs/crate/tungstenite/0.24.0/source/src/protocol/frame/mod.rs; https://docs.rs/crate/tokio-rustls/0.26.4/source/src/common/mod.rs. Reference: Cargo.lock:4245; Cargo.lock:4467; crates/shamir-transport-ws/src/server.rs:43.
- **Receive operations are zero-allocation and the complete transport is lock-free** — `diverges`. Only sufficiently large caller scratch avoids adapter reallocation. Exact tungstenite 0.24.0 allocates frame/message storage; production creates fresh receive Vecs. futures-util 0.3.32 split halves use BiLock, which releases its guard after each poll but is still synchronization: https://docs.rs/crate/futures-util/0.3.32/source/src/stream/stream/split.rs. Reference: crates/shamir-transport-ws/src/framing.rs:87; crates/shamir-server/src/connection/request_loop.rs:278; Cargo.lock:1469.
- **Native binding mode and protocol version are checked independently of WS subprotocol negotiation** — `supported`. Full authentication compares the requested mode with listener policy, requires V1, and includes mode/exporter in the transcript. Native zero fallback does not silently change mode to browser. Reachable failure of the real post-handshake exporter was not established. Reference: crates/shamir-connect/src/server/handshake.rs:135; crates/shamir-connect/src/server/handshake.rs:265.
- **Spec close codes and server-initiated heartbeat are implemented** — `diverges`. TEXT/malformed/oversize errors lose close-code identity and shutdown uses close(None). No server Ping timer or Pong-response deadline exists. The existing 600-second connection idle timeout is not the promised heartbeat. The specification must not prescribe transmitting reserved code 1006; [RFC 6455 §7.4.1](https://www.rfc-editor.org/rfc/rfc6455#section-7.4.1) prohibits that. Reference: docs/guide-docs/client-server-protocol-spec/TRANSPORT_WS.md:57; docs/guide-docs/client-server-protocol-spec/TRANSPORT_WS.md:70; crates/shamir-server/src/framer.rs:360.
- **Plain WS is removed from v1** — `diverges`. The public transport still documents and permits PlainWsLoopback, while production skips unsupported WS/plain combinations. This is a stale public contract, not evidence of a deployed non-loopback plaintext listener. Reference: docs/guide-docs/client-server-protocol-spec/TRANSPORT_WS.md:3; crates/shamir-transport-ws/src/listener.rs:29; crates/shamir-server/src/server/server_launcher.rs:713.
- **Browser authentication coverage exists and WS authentication audit labels identify the transport** — `supported`. Conditional Node-driven TS tests reach the browser-profile production seam; auth audit emission maps WebSocket to ws. Positive integration does not detect removal of Origin rejection, and no test execution is asserted. Reference: crates/shamir-client-ts/src/__tests__/connect.test.ts:194; .github/workflows/ts-e2e-nightly.yml:85; crates/shamir-server/src/connection/handshake.rs:49.
- **Root exports, opaque framing and documented browser trust limits** — `supported`. ws_recv is root-exported and constants remain publicly namespaced; no all-items-at-root obligation exists. Framing accepts opaque payloads without needing MessagePack deserialization. Browser relay limitations are documented; arbitrary malformed Origin witnesses do not establish ambient-cookie authentication or normal-browser exploitation. Reference: crates/shamir-transport-ws/src/lib.rs:24; crates/shamir-transport-ws/src/lib.rs:31; docs/guide-docs/client-server-protocol-spec/SECURITY_MODEL.md:109.

## Reviewer's prior-cycle comparison

These are the independent reviewer's comparisons before parent refinements; the accepted ledgers above govern final decisions and counts.

- SUMMARY.md#7.2: confirmed-open -&gt; refuted. ws_recv was already exported in initial commit 3653540c, MAX_WS_FRAME_SIZE remains publicly accessible, and no root-export completeness contract exists.
- error-handling-lifecycle.md#5 and SUMMARY.md#6.5: confirmed-open -&gt; not-applicable. Initialized scratch retention on Err violates no documented postcondition, and inspected production callers do not decode after failure.
- SUMMARY.md#P1.8: unverified -&gt; confirmed-open. Exact checksummed tungstenite 0.24.0 source establishes existing write-buffer configuration and encoded Pong accumulation; exact tokio-rustls/rustls source supports continued deployed reads under pending output.
- api-wire-protocol.md#1 and SUMMARY.md#5.1: open status retained, severity medium -&gt; high for browsers actually offering shamir-v1. WHATWG §2.2 establishes mandatory failure without server acknowledgement; the prior unverified consequence is resolved.
- performance-hotpath.md#1 and SUMMARY.md#4.1: open mechanism retained, severity medium -&gt; low. Source proves avoidable allocation/copy but not the claimed runtime impact.
- security-crypto.md#3 and SUMMARY.md#3.3: reflection retained as nit rather than low security severity; no exploitable rendering or log-control witness was established.
- api-wire-protocol.md#5 and SUMMARY.md#5.5: semantic distinction retained as a documentation nit. Shared byte layout does not mandate identical close semantics, and rejecting zero is not TCP graceful-close alignment.
- Previously refuted zero-browser-coverage and missing-ws_recv claims remain refuted by pre-existing evidence. No assigned claim was closed merely because of historical green labels.
- The new receive-boundary, native-Origin, close-code and heartbeat findings contradict broad contract-completeness assurances but are not an exhaustive fresh audit.
- The working lockfile changed during this read-only review. Frozen pins were reconfirmed from the required commit; no result relies on the external upgrades and no file was modified by this reviewer.

## Current follow-up order

1. Bound the unauthenticated client_proof stage and backpressured output/cleanup using the existing pinned APIs; preserve legitimate control handling and connection-budget release.
2. Resolve configured-path behavior and implement compatible shamir-v1 negotiation across server and clients, with exact upgrade oracles.
3. Reconcile logical payload limits with four-byte codec headroom and larger server response settings; add before-allocation outbound rejection.
4. Complete endpoint Origin policy, correct missing-Origin status, and add live negative/status tests that fail if validation is removed.
5. Preserve specified WS close codes and implement the promised heartbeat with bounded teardown; never transmit reserved code 1006.
6. Fail closed on native exporter extraction errors while preserving diagnostics and binding-mode identity.
7. Repair ledger/document overstatements and unused dependencies; optimize owned sends/policy sharing only with coordinated ownership semantics and scoped measurement.
8. Keep optional root exports, error-buffer clarification, redundant helpers and import cleanup separate from runtime/security remediation.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-transport-ws — Consolidated 7-lens review (synthesis of the 2026-08-14 cross-crate sweep)

Crate: `crates/shamir-transport-ws/` — the WebSocket transport binding (tokio-tungstenite 0.24):
native WSS `/shamir/v1` + browser WSS `/shamir/v1/browser` with length-prefixed framing
(`[u32_be length][payload]` per BINARY message), Origin-policy-gated browser handshakes,
TLS-exporter extraction for SCRAM channel binding, and WS listener profiles. This is the
WSS front door for `shamir-server` (browser endpoint included).

Review basis — synthesis (not a fresh review) of the seven lens files produced by the
2026-08-14 23-crate × 7-lens sweep, all carried forward in full and deduped across lenses:

- `correctness-tdd.md` · `concurrency-lockfree.md` · `security-crypto.md` ·
  `performance-hotpath.md` · `api-wire-protocol.md` · `error-handling-lifecycle.md` ·
  `style-claude-md.md` (same directory)

Structure/tone/rigor calibrated against the two exemplar syntheses:
`../shamir-client-node/SUMMARY.md` and `../shamir-transport-ipc/SUMMARY.md` (the latter a
sibling transport crate). The workspace-wide `SUMMARY.md` rows for this crate (49
lens-tagged findings: 0c/3h/11m/21l/14n; verdict *"moderate — spec/interop gaps and an
untested security control"*) were used as a cross-check only — the independent recount of
the seven files below matches them exactly.

Method: read-only. No build/test/lint commands were run; no source file under `crates/` was
modified. Every load-bearing file:line citation was spot-checked against the current tree
(`server.rs`, `framing.rs`, `browser.rs`, `tls_exporter.rs`, `listener.rs`, `lib.rs`,
`Cargo.toml`, `tests/framing_round_trip.rs`); one source-file inaccuracy was found and is
corrected inline at 7.2. No new defects surfaced during spot-checking.

Finding numbering below preserves each lens file's own numbering (1.1 = correctness #1, …)
so every entry is traceable to its source. Where the same root-cause defect was flagged by
multiple lenses, the full write-up lives once under its primary lens and the other lenses
carry a short `*(dedup — primary: X.Y)*` stub.

## Executive summary

Structurally this is one of the cleanest crates in the workspace — lock-free by
construction, zero `unsafe`, `thiserror`-only errors, exemplary test/layout conformance —
but it is not shippable as documented: (1) **the endpoint paths are hardcoded string
literals while the server treats `path` as operator configuration** — a reconfigured
listener boots cleanly and then 404s every upgrade, total silent connectivity loss (5.2);
(2) **the spec §2.1 `Sec-WebSocket-Protocol: shamir-v1` negotiation is entirely
unimplemented** — a spec-conformant browser client fails the connection itself after the
101, and the spec's mismatch→400 downgrade defense never fires (5.1); (3) **the browser
endpoint's primary anti-CSWSH control (Origin enforcement) is wired by zero tests anywhere
in the workspace**, while the typed `WsAcceptError` taxonomy advertises Origin/path
classification it can never deliver (1.1, 6.1) — so the security control can be silently
regressed green. Fix those three, plus the one-line manifest fix that removes the phantom
`tungstenite 0.29` parser copy (3.1), before anything else ships from this crate.

---

## 1. correctness-tdd

### 1.1 [HIGH] `accept_browser_ws` — the spec §9 Origin enforcement path — has zero test coverage *(primary of the accept-wiring test group; also 3.2, 5.6, 6.2)*
- **File:line:** `crates/shamir-transport-ws/src/server.rs:108-146`; absent from
  `src/tests/server_tests.rs` (native-only) and `src/tests/browser_tests.rs` (pure
  `validate_origin` only); confirmed workspace-wide: only non-test reference is
  `shamir-server/src/server/server_launcher.rs:1266`.
- **Issue:** Only the pure function `validate_origin` is tested. Nothing tests the handshake
  wiring it is embedded in: the header extraction (`req.headers().get(ORIGIN)` +
  `to_str().ok()`, where a non-UTF-8 header silently maps to `None`→`Missing`), the 403
  `ErrorResponse` on rejection, the wrong-path 404, or that the policy actually gates the 101
  upgrade. `shamir-server/tests/mvp_ws_e2e.rs` exercises only `/shamir/v1` (native). The
  repo's own NEW-1 work sets the standard of live-wiring tests for accept-path security caps
  (`live_accept_rejects_message_over_cap`); the Origin control never got the equivalent.
- **Failure scenario:** A refactor of the callback (inverted check, dropped `validate_origin`
  call, dropped `move` capture falling back to an empty default policy, path-string typo,
  header name change, validation moved after the upgrade response) silently disables the
  primary cross-site WebSocket-hijacking defence — and the entire suite stays green.
- **Suggested fix:** Add live handshake tests in `src/tests/server_tests.rs` mirroring the
  existing `live_accept_*` duplex pattern: (a) allowed Origin → upgrade succeeds (101 + frame
  round-trip); (b) missing Origin → handshake fails with HTTP 403; (c) disallowed Origin →
  403, no 101; (d) wrong path on both endpoints → 404; (e) non-UTF-8 Origin value → rejected.

### 1.2 [MEDIUM] Framing's malformed-input error paths untested; `rejects_oversized_frame` is under-asserted *(primary of the framing-error-path test group; also 5.6, 6.2)*
- **File:line:** `tests/framing_round_trip.rs:57-71` (asserts only `is_err()` at :69);
  `crates/shamir-transport-ws/src/framing.rs:148-153` (short-message `LengthMismatch`),
  `:157-162` (prefix-tamper `LengthMismatch` — the module doc's headline "defence-in-depth"
  invariant, framing.rs:7-9), `:173` (`PeerClose` on Close frame), `:144` (`PeerClose` on
  stream end), `:177-182` (`NonBinaryMessage` for TEXT), `:176` (Ping/Pong skip loop) — none
  tested.
- **Issue:** The only negative framing test sends a valid 200-byte frame and receives with a
  tiny cap, then asserts merely `result.is_err()`. It would still pass if the error were an
  unrelated `Io`/`Capacity` error, i.e. it does not pin the `TooLarge` contract. Every tamper
  path the module doc advertises (mismatched inner prefix, sub-4-byte message, TEXT
  rejection) is dead code as far as the suite can prove. Note these are pure logic over an
  already-assembled `Message` — trivially testable without raw-frame crafting.
- **Failure scenario:** A reorder of the `declared != body.len()` vs `TooLarge` checks, a
  flipped comparison, an accidental `Ok(())` on the TEXT arm, or a `continue` added to the
  TEXT arm would not be caught by any test.
- **Suggested fix:** (a) Assert `matches!(result, Err(WsFrameError::TooLarge { .. }))` (and
  the declared/actual fields). (b) Add unit-style tests through `ws_recv_into_stream` over a
  custom `Stream` yielding crafted `Message`s: `Binary(vec![0,0,0,9, ...])` with wrong body,
  2-byte binary, `Text`, `Close`, then `None` — one per error variant.

### 1.3 [LOW] *(dedup — primary: 6.1)* `WsAcceptError::WrongPath` never constructed; `OriginRejected(#[from])` unreachable
Full write-up at 6.1 (the error-handling lens rated it medium; this lens low). The
correctness-angle detail: verified `WsAcceptError` appears only in `server.rs`/`lib.rs`;
`shamir-server` merely logs `?e` — no caller anywhere can match the dead variants today.

### 1.4 [LOW] *(dedup — primary: 5.3)* `ws_send_sink` has no send-side cap; `payload.len() as u32` silently truncates
Full write-up at 5.3. Receiver-side containment (the mismatch check at framing.rs:157
rejects the corrupt frame) was this lens's contribution: system damage is a confusing
remote `LengthMismatch` instead of a local error, not desync.

### 1.5 [LOW] Wildcard origin matcher is raw string logic — accepts literal `*` and userinfo forms; unvalidated patterns fail silently *(primary of the origin-matcher group; also 3.8, 5.10)*
- **File:line:** `crates/shamir-transport-ws/src/browser.rs:56-76` (`origin_matches`),
  `:37-41` (`allow` performs no pattern validation).
- **Issue:** (a) The `*` label itself matches literally: `Origin: https://*.example.com` is
  accepted by pattern `https://*.example.com` (after_scheme = `*.example.com`, first dot is
  at index 1, suffix compares equal). (b) Userinfo-bearing origins pass:
  `https://user@app.example.com` matches `https://*.example.com` because the "first dot"
  split ignores the `@`. (c) A pattern without a scheme (`*.example.com`) contains no
  `//*.`, falls through to exact comparison, and therefore matches *nothing* — a config
  typo silently narrows the allowlist with no error at construction. Browsers never
  serialize origins in forms (a)/(b), so the CSRF defence holds in practice (and failures
  are fail-closed), but the matcher accepts origins a strict host-label match would reject.
  The api lens adds: `origin_matches` is case-sensitive byte comparison (RFC 6454 makes
  scheme/host case-insensitive), and wildcard detection is an unanchored whole-pattern
  `find("//*.")` scan with no port/trailing-dot normalization.
- **Failure scenario:** An allowlisted user whose environment emits a mixed-case Origin is
  rejected; operators "fix" it by adding both casings to the allowlist, accumulating
  near-duplicate entries. Independently, a typo'd pattern boots cleanly and full-rejects —
  surfaced only via client-side 403s.
- **Suggested fix:** In `origin_matches`, reject `after_scheme.starts_with('*')`, validate
  the first component as a non-empty DNS label without `/`, `@`, `:`, or `?`, ASCII-lowercase
  both sides, and anchor the wildcard check to immediately after `scheme://`. In `allow`,
  reject (or `debug_assert`) patterns lacking `<scheme>://` — or add `try_allow -> Result` —
  so operator typos surface at boot.

### 1.6 [LOW] *(dedup — primary: 3.1)* Direct `tungstenite = "0.29"` dependency is unused and version-skewed vs the effective 0.24
Full write-up at 3.1 (five lenses flagged this one dependency line).

### 1.7 [NIT] `BrowserOriginPolicy::empty()` doc references a nonexistent `accept_no_origin` mode
- **File:line:** `crates/shamir-transport-ws/src/browser.rs:28-30`.
- **Issue:** The doc says the empty policy "rejects everything except the explicit
  `accept_no_origin = true` mode (which is for testing only)". No such mode or parameter
  exists anywhere in the crate — `validate_origin` (browser.rs:95-105) unconditionally
  rejects a missing origin, and `accept_browser_ws` exposes no bypass. Spot-checked against
  the current tree: confirmed stale.
- **Suggested fix:** Delete the stale sentence; if an escape hatch is ever added, document
  it then.

### 1.8 [NIT] *(dedup — primary: 3.5)* `BROWSER_CHANNEL_BINDING` is dead; the zeros invariant is encoded twice
Full write-up at 3.5 (covers the dead const, the re-hardcoded `[0u8; 32]` literals, and the
`unwrap_or`-invitation API shape as one root defect).

### 1.9 [NIT] *(dedup — primary: 4.4)* Error-semantics / doc-accuracy warts
Full write-up at 4.4 (the pre-auth-cap misattribution group; includes this finding's
`LengthMismatch::actual` field-doc half — framing.rs:148-153 reports `bytes.len()` for
sub-4-byte messages while framing.rs:42-43 documents "body length minus 4").

## 2. concurrency-lockfree

**General verdict: clean.** Reading all six source files plus a crate-wide grep found zero
`std::sync::Mutex`/`RwLock`/`parking_lot`, zero atomics, and no `scc`/`dashmap`/hash-map
surface at all — so there are no locks held across `.await`, no `scc::*::len()` sites, and
no Fx-hash default to violate, by definition. All potentially shared state is avoided
through ownership (`&mut` free-fn parameters, `split()` halves); every I/O op is
`async fn` — exactly the pillar-1/2 shape. Coverage of the concurrency claims (split-half
duplex under real tokio tasks, live accept-path wiring) is present and adequate. The
theme's two findings are O(x→0) items:

### 2.1 [LOW] *(dedup — primary: 4.1)* Send framing ships only the allocating variant — production send path allocates + memcpys per message while recv is zero-alloc
Full write-up at 4.1. This lens's framing: the asymmetry is sharpest *within* the crate —
`ws_recv_into`/`ws_recv_into_stream` write into caller scratch (zero-alloc steady state,
framing.rs:92/132), so a fully pooled per-connection duplex loop is impossible on WS; and
the doc's parity claim with the TCP sibling (framing.rs:62) overstates — TCP ships three
variants (`write_frame` / `write_frame_into` / `write_frame_prereserved`,
shamir-transport-tcp/src/framing.rs:149/:187/:233), WS ships one.

### 2.2 [LOW] `accept_browser_ws` deep-clones the entire origin allowlist per accepted connection *(primary of the policy-clone group; also 5.9, 4.5)*
- **File:line:** `crates/shamir-transport-ws/src/server.rs:118` (`let policy =
  policy.clone();`; type at `src/browser.rs:23-25`; caller
  `shamir-server/src/server/server_launcher.rs:1266`).
- **Issue:** Deep-copies `BrowserOriginPolicy` (a `Vec<String>`: 1 + P heap allocations,
  P = number of configured origins) on **every** accepted browser connection solely to move
  owned state into the `move` handshake callback — per-op state duplication for a value
  that is immutable for the handshake's lifetime. The pillars ask for read-shared config to
  move by refcount, not by deep copy (pillar 3 / pillar 5's single-writer-many-reader →
  `Arc` guidance; no lock is involved). The api lens adds: tungstenite's `Callback` trait
  carries no `'static` bound, so the closure could equally borrow `&'a BrowserOriginPolicy`
  with an explicit lifetime on the async fn — the per-connection clone in `server_launcher`
  (which clones at :1240 to move into the spawned task) would then be the only copy.
- **Failure scenario:** None for correctness; wasted allocation + linear copy on the
  per-connection accept path — amortizes to nothing at low churn, visible at high connection
  churn and/or large operator allowlists, and invites the same clone to propagate outward
  as "precedent."
- **Suggested fix:** Take `policy: &Arc<BrowserOriginPolicy>` (or add an
  `accept_browser_ws_arc` overload, or a borrowed-`'a` signature) and clone the `Arc` into
  the closure — one atomic refcount bump instead of 1 + P allocations. The sole production
  caller already builds the policy once at boot (`browser_origin_policy_from`,
  server_launcher.rs:924-930) and holds it per-listener, so switching that field to
  `Arc<BrowserOriginPolicy>` is mechanical.

No further findings for this theme.

## 3. security-crypto

**Boundary verdict: largely sound.** Zero `unsafe` (grep-verified); Origin is validated
inside tungstenite's handshake callback **before** the 101 response; endpoint paths
exact-match; framing enforces `declared == actual` as defense-in-depth; the NEW-1 pre-auth
buffering cap is pinned to 16 MiB and live-tested; the TLS-exporter boundary fails closed
on the client side. No timing-sensitive comparisons exist here (Origin matching guards no
secret; SCRAM/HMAC and the binding_mode anti-downgrade matrix live in shamir-connect, as
the crate docs correctly state). Checked and clean this theme: TEXT rejection reflects no
payload content (`TEXT len=N` only); non-ASCII Origin classifies as `Missing` (fail-closed);
TLS 1.3-only is delegated to `shamir-transport-tcp`'s rustls config rather than duplicated.

### 3.1 [MEDIUM] Phantom `tungstenite = "0.29"` dependency — two WS parsers compiled, the live one is the older *(primary of the phantom-dependency group; also 5.4, 4.3, 1.6, 7.4)*
- **File:line:** `crates/shamir-transport-ws/Cargo.toml:20` (evidence: `Cargo.lock:4233-4246`,
  `4455-4488`, `3769`).
- **Issue:** Every code path imports WS types via the `tokio_tungstenite::tungstenite::*`
  re-export — i.e. tungstenite **0.24.0**, the version tokio-tungstenite 0.24.0 pins. The
  direct `tungstenite = "0.29"` entry is never imported anywhere under `src/` (verified by
  grep). Cargo.lock consequently carries **both** tungstenite 0.24.0 and 0.29.0, and
  `shamir-transport-ws` is the sole depender of the 0.29.0 copy. The public API leaks 0.24
  types: `WsFrameError::Io(#[from] tokio_tungstenite::tungstenite::Error)` (framing.rs:33)
  and all `Message` handling — written against 0.24 semantics (`Message::Binary(Vec<u8>)`,
  vs `Bytes` in newer tungstenite). Five of the seven lenses independently flagged this
  line (the security reviewer's CVE-false-assurance angle is the sharpest statement of it).
- **Failure scenario:** A CVE fix or hardening release on the tungstenite 0.29 line gives
  false assurance — `cargo audit` triage and auditors see 0.29.0 present while the code
  that actually parses untrusted frames from unauthenticated peers runs 0.24.0 and receives
  nothing. A maintainer "responding to the advisory" by bumping the manifest line fixes
  nothing. A second full parser copy (plus its `rand 0.9` / `thiserror 2` closure) is also
  compiled into the binary for no benefit; and a future `use tungstenite::Message` would
  compile against 0.29 types that do not unify with the `WebSocketStream`'s 0.24 types.
- **Suggested fix:** Delete the `tungstenite = "0.29"` line; if tungstenite types are ever
  needed directly, use the `tokio_tungstenite::tungstenite` re-export (as the code already
  does) so exactly one version exists. Treat "move to a tokio-tungstenite release on the
  0.29 line" as its own deliberate upgrade task. While there, the direct `rustls` /
  `tokio-rustls` entries (Cargo.toml:23-24) are likewise unused by this crate's code (types
  arrive via `shamir_transport_tcp::tls::ConnectionExporter`); dropping them keeps the
  manifest honest about the crypto boundary.

### 3.2 [MEDIUM] *(dedup — primary: 1.1)* `accept_browser_ws` Origin enforcement has no live-wiring test coverage
Full write-up at 1.1. The security-angle framing: this is the only consumer path that turns
`validate_origin` into a security control, and it is exercised by **no test anywhere in the
workspace** (`accept_browser_ws` appears only in `server.rs`, `lib.rs`, and the production
caller `server_launcher.rs:1266`).

### 3.3 [LOW] Attacker-controlled `Origin` echoed into the HTTP 403 response body
- **File:line:** `crates/shamir-transport-ws/src/server.rs:136` (source: `browser.rs:87`,
  `browser.rs:103`).
- **Issue:** `OriginRejected::NotAllowed(origin.to_string())` embeds the raw header value,
  and the accept path interpolates it into the `ErrorResponse` body sent on the wire
  ("origin rejected: {rej}"). Exploitation as XSS is effectively blocked in practice
  (rendering the body requires a top-level navigation, which does not carry an
  attacker-chosen `Origin`; fetch/WS callers cannot read the cross-origin body), but it is
  needless reflection of untrusted input in a boundary response, and the same unsanitized
  string also flows into operator debug logs (`ws browser upgrade failed`,
  server_launcher.rs:1270).
- **Failure scenario:** Log/wire injection surfaces crafted origin text in operator
  consoles and HTTP responses; no code-execution path identified today.
- **Suggested fix:** Send a static body ("origin rejected") and keep the offending value in
  structured `tracing` fields only; `OriginRejected` already preserves it for library
  callers.

### 3.4 [LOW] Unbounded control-frame loop in `ws_recv_into_stream` (ping-flood liveness); auto-pong write queue is uncapped *(primary of the control-frame-flood group; also 4.2)*
- **File:line:** `crates/shamir-transport-ws/src/framing.rs:176`, `:183` (Ping/Pong/Frame
  `continue`); `src/server.rs:42-51` (`server_ws_config` deliberately leaves
  `max_send_queue` at default, per the deprecation comment at :43-45).
- **Issue:** `ws_recv_into_stream` loops over PING/PONG/Frame messages with no
  per-connection budget. Tungstenite auto-queues a Pong for every PING read; in the
  split-half layout shamir-server actually deploys (`WsFrameReader`/`WsFrameWriter` over
  `StreamExt::split`), those pongs sit in the shared write queue that only the writer task
  drains. A hostile peer that streams PINGs without reading keeps the read loop spinning
  indefinitely *and* drives the connection's outgoing buffer to grow without a crate-level
  cap (`max_send_queue` intentionally at tungstenite's default; the 16 MiB message caps in
  `server_ws_config` do not cover this). Production callers currently wrap reads in
  `auth_init_timeout`-style bounds, but the crate API itself offers no progress guarantee —
  any future `ws_recv*` caller without an outer timeout can be wedged one task per
  connection, forever.
- **Failure scenario:** An unauth'd (or auth'd) peer pins server memory per connection via
  ping-flood + no-read (slow-reader); multiplied by many peers this is a memory-pressure
  DoS vector the NEW-1 hardening does not cover, plus 1:1 forced outbound pong traffic.
- **Suggested fix:** Cap consecutive non-BINARY messages (4-8, or a 64-class budget) and
  return a dedicated `WsFrameError::ControlFrameFlood`, matching the crate's fail-closed
  style; and/or document/enforce a write-buffer cap for the pinned tokio-tungstenite 0.24
  (verify the 0.24 default; 0.26+ replaced `max_send_queue` with bounded
  `write_buffer_size` options — a version bump also fixes it structurally). Pair with an
  idle/dead-peer timeout at the session layer.

### 3.5 [LOW] `Option`-returning exporter API + public all-zeros constant invites silent zero-substitution on the native path *(primary of the exporter-placeholder group; also 6.4, 5.8, 1.8, 7.3)*
- **File:line:** `crates/shamir-transport-ws/src/tls_exporter.rs:20-25`; consumption sites
  `shamir-server/src/server/server_launcher.rs:1073`, `:1159` (`unwrap_or([0u8; 32])` on the
  **native** path), `:1263` (`let exporter = [0u8; 32];`); the crate's own doc prose
  (`lib.rs:11-12`, `server.rs:11`).
- **Issue:** `extract_tls_exporter_from_stream -> Option<[u8; 32]>` plus the exported
  `BROWSER_CHANNEL_BINDING = [0u8; 32]` makes `unwrap_or(<zeros>)` the path of least
  resistance — which is exactly what the production callers do today, including on the
  **native** path where binding_mode = 0x01/TlsExporter. Today this fails closed only by
  accident of the client also failing closed on `None` (`shamir-client/src/client.rs:391-392`,
  `:603-604`): zero-vs-real binding bytes break the SCRAM proof. But it masks a broken TLS
  state as a generic auth failure, and the design is one client-side `unwrap_or` away from
  a native session genuinely bound with the browser placeholder. Compounding it, the
  exported `BROWSER_CHANNEL_BINDING` const — which exists precisely to single-source the
  "browser exporter = zeros per spec §6.4" invariant — has **zero references in the entire
  workspace**: it is not re-exported in `lib.rs`, not used by `accept_browser_ws`, not
  tested, while `server_launcher` re-hardcodes the literal twice (two encodings of a
  protocol-mandated value = drift hazard). The error-handling lens adds: the `Option`
  shape swallows the `rustls::Error` cause — on the native endpoint the exporter always
  exists post-handshake (TLS 1.3 always supports it, per
  `shamir-transport-tcp/src/tls.rs:74-76`), so `None` is an error, not an absent value.
- **Failure scenario:** A native WSS caller maps `None` to the zero placeholder: SCRAM
  channel binding silently weakens with no error surfaced, and the protocol's
  anti-downgrade matrix will not flag it because 0x02 is a legal mode. Independently, if
  the placeholder semantics ever change, the literal sites drift from the unused const.
- **Suggested fix:** Expose one fail-closed helper, e.g. `channel_binding_for(stream,
  BindingMode) -> Result<[u8; 32], ChannelBindingError>`, which errors when extraction
  fails for `TlsExporter` and returns the placeholder only for `TlsNoExport` (carrying the
  rustls cause, per 6.4); use `BROWSER_CHANNEL_BINDING` at both server_launcher sites (it
  is already exported at `shamir_transport_ws::tls_exporter::`); make the native path
  return an error (or at least a loud warn + explicit downgrade marker) on extraction
  failure rather than quietly reusing the browser placeholder; and gate/rename the const
  (`TLS_NO_EXPORT_PLACEHOLDER`) with a doc warning against `unwrap_or` on native paths.

### 3.6 [NIT] *(dedup — primary: 4.4)* Doc misattributes the 4 KiB pre-auth cap to this crate's framing layer
Full write-up at 4.4.

### 3.7 [NIT] *(dedup — primary: 5.3)* `ws_send_sink` truncates the length prefix for payloads >= 4 GiB
Full write-up at 5.3 (equality is enforced on both sides, so no desync or memory-safety
issue — only a confusing remote protocol error; `u32::try_from` costs nothing).

### 3.8 [NIT] *(dedup — primary: 1.5)* `BrowserOriginPolicy::allow` accepts malformed patterns that silently never match
Full write-up at 1.5. Fail-closed, but a configuration trap surfaced only via client-side
403s; validate at construction (`try_allow -> Result`) or `debug_assert!` the shape
(must contain `://`; at most one `*`, only in the `//\*.` slot).

## 4. performance-hotpath

Scope note: judged against pillar 3 (O(x→0)) and pillar 1 (no hot-path locks), with
consumer context `shamir-server/src/framer.rs` + `connection/request_loop.rs`, which drive
`ws_send_sink` / `ws_recv_into_stream` once per request/response frame on the WSS path.
**The receive path is genuinely O(x→0)-clean:** `ws_recv_into_stream` recycles the caller's
scratch buffer (`clear` + `extend_from_slice`), capacity reuse is proven by
`tests/framing_round_trip.rs::round_trip_into_buffer_reuses_capacity`, validation is all
constant-time checks, `BrowserOriginPolicy::allows` is a linear scan over an
operator-config allowlist (constant w.r.t. traffic), and error-path `format!`/`to_string()`
allocations fire only on rejection paths. Not findings (checked clean): recv-path
allocation, hidden O(N) scans, pillars 1/3/5. The send path is the weak side:

### 4.1 [MEDIUM] WSS send hot path: fresh heap alloc + full-payload copy per frame; TCP's prereserved zero-copy path is silently defeated *(primary of the send-alloc group; also 2.1)*
- **File:line:** `crates/shamir-transport-ws/src/framing.rs:114-124` (`ws_send_sink`; parity
  claim in doc at `:62`); consumer context `shamir-server/src/framer.rs:110-127, 348-365,
  405-415`, `shamir-server/src/connection/request_loop.rs:198-213`.
- **Issue:** `ws_send_sink` takes `&[u8]` and, on every call, allocates
  `Vec::with_capacity(4 + payload.len())` and memcpys the entire payload into it
  (framing.rs:119-121) before handing it to tungstenite (which copies it again into its
  internal write buffer). The server's request loop builds every response as an already
  length-prefixed buffer precisely to avoid a memcpy — `write_frame_prereserved` — and TCP
  overrides it with `tcp_write_frame_prereserved` (zero copy). The WS `FrameWriter` does
  not override it, so the trait's default strips the 4-byte prefix and routes through
  `ws_send_sink`: the caller's prereserved copy is wasted and a second full-payload copy
  into a brand-new heap `Vec` is paid on **every WSS response frame**. The
  `Framer::write_frame_into` `scratch` parameter, designed for exactly this zero-alloc
  reuse, is explicitly ignored by the WS impl (`_scratch`, framer.rs:355/408 — "WS already
  builds its own send buffer (one allocation per message)"), i.e. one malloc+free per frame
  on the hottest WSS loop, plus a redundant O(payload) memcpy; at the 16 MiB frame ceiling
  that is two extra 16 MiB traversals per frame.
- **Failure scenario:** Throughput/latency tax on all WSS traffic (browser endpoint
  included) proportional to payload size; allocator churn under high RPS. Not asymptotically
  worse than the transport itself, so medium rather than high.
- **Suggested fix:** Mirror the TCP trio where it maps onto tungstenite's owned
  `Message::Binary(Vec<u8>)` model: (a) add an ownership-taking variant, e.g.
  `ws_send_sink_vec(sink, Vec<u8>)` — a caller that yields its prereserved buffer moves it
  with **zero** copy, since tungstenite copies into its internal write buffer at flush
  regardless; this is the variant that actually removes the extra copy — then override
  `write_frame_prereserved` in shamir-server's `WsFrameWriter` to use it. (b) Optionally a
  scratch-reusing variant matching the `Framer` scratch contract, and/or an owned-`Vec`
  variant that reserves and prepends the header in place (`Vec::splice(0..0, …)`) to drop
  the per-send allocation. Do **not** copy `write_frame_into`'s scratch-buffer signature
  blindly — `Message::Binary` takes ownership, so a pooled scratch would need a `clone()`
  (same memcpy, zero gain). Document the copy semantics on `ws_send_sink`.

### 4.2 [LOW] *(dedup — primary: 3.4)* Receive loop consumes unbounded consecutive control frames; auto-pong write queue is uncapped
Full write-up at 3.4 (this lens contributed the outbound write-queue growth analysis and
the tokio-tungstenite 0.26+ `write_buffer_size` note).

### 4.3 [LOW] *(dedup — primary: 3.1)* Dead direct dependency `tungstenite = "0.29"` compiles a second, unused copy of tungstenite
Full write-up at 3.1. This lens's angle: none at runtime — wasted CI/dev build time,
binary bloat, and a latent type-mismatch hazard (`Cargo.lock:3769` shows this crate as the
sole depender of tungstenite 0.29.0).

### 4.4 [LOW] Doc drift: `server_ws_config` claims a "4 KiB pre-auth logical check … enforced in `crate::framing::ws_recv_into`" — no such enforcement exists in this crate *(primary of the pre-auth-doc group; also 3.6, 6.6, 1.9)*
- **File:line:** `crates/shamir-transport-ws/src/server.rs:26-28` (and mirrored in
  `src/tests/server_tests.rs:11-13`); the doc's own Residual paragraph at :33-41 says the
  opposite, contradicting the earlier sentence.
- **Issue:** framing.rs has no pre-auth constant; the ceiling is entirely the
  caller-supplied `max_frame_size` parameter, and the actual 4 KiB pre-auth enforcement
  lives in shamir-server (`connection/handshake.rs` passing `MAX_PRE_AUTH_FRAME`, with the
  constant itself in shamir-connect; enforcement points framer.rs:341/399). The doc's
  cross-reference implies the transport crate self-enforces the 4 KiB pre-auth budget; it
  does not — tungstenite will have buffered up to the full 16 MiB before any logical check
  runs. Misdocuments where the guard lives for future perf/security work. Two companion
  doc warts ride in the same group: (a) correctness 1.9 — for a <4-byte message the error
  reports `actual: bytes.len()` while `LengthMismatch::actual`'s field doc (framing.rs:42-43)
  says "WS message body length minus 4"; (b) error 6.6 — an error-enforcement claim
  pointing at the wrong layer invites future verification in the wrong file.
- **Failure scenario:** A future refactor trusts the comment, drops/changes the
  caller-supplied pre-auth cap, and silently re-widens unauthenticated buffering to
  16 MiB/peer — reinstating exactly what NEW-1 fixed.
- **Suggested fix:** Reword to "enforced by the caller (shamir-server passes
  `MAX_PRE_AUTH_FRAME`); this crate only bounds tungstenite's buffering via
  `server_ws_config`" (also at server_tests.rs:11-13); either special-case the
  short-message error or amend the `LengthMismatch::actual` field doc.

### 4.5 [NIT] *(dedup — primary: 2.2)* `accept_browser_ws` deep-clones the origin policy per connection accept
Full write-up at 2.2. This lens's severity call: O(allowlist) per *connection* (constant
w.r.t. traffic), so minor — but trivially avoidable.

## 5. api-wire-protocol

The framing, origin-policy, and listener-profile APIs are clean and well-documented, and
this crate constructs no queries or raw JSON anywhere, so the builder-only rule is
satisfied by construction. The wire-protocol problems are at the handshake and
cross-transport seams:

### 5.1 [HIGH] Spec-mandated WebSocket subprotocol negotiation is unimplemented
- **File:line:** `crates/shamir-transport-ws/src/server.rs:85-99` and `:120-143` (both
  accept callbacks); spec: `docs/guide-docs/client-server-protocol-spec/TRANSPORT_WS.md:18`
  (§2.1).
- **Issue:** Spec TRANSPORT_WS §2.1 is normative for this crate (every module cites the
  spec by section): the client sends `Sec-WebSocket-Protocol: shamir-v1`, the server
  "confirm[s] same. Mismatch → 400." Neither `accept_native_ws` nor `accept_browser_ws`
  reads or echoes the `Sec-WebSocket-Protocol` header — the handshake callbacks check only
  the URI path and (browser) `Origin`. There is no transport-layer protocol-version gate at
  all.
- **Failure scenario:** A spec-conformant browser client that requests `shamir-v1` gets a
  101 response with no echoed `Sec-WebSocket-Protocol`; per RFC 6455 the browser then
  *fails the connection itself* ("Incorrect 'Sec-WebSocket-Protocol' header") — a
  conformant client cannot connect at all. Conversely, a client offering a different/future
  subprotocol (`shamir-v2`) is silently accepted, so the spec's mismatch→400 downgrade
  defense never fires. First-party clients dodge both today only because they also ignore
  the spec (`shamir-client-ts/src/platform/browser.ts:118` calls `new WebSocket(url)` with
  no subprotocol) — implementation and spec have drifted in opposite directions.
- **Suggested fix:** In both handshake callbacks, read `Sec-WebSocket-Protocol`: reject with
  400 unless it contains exactly `shamir-v1`, and echo `shamir-v1` back via the callback's
  `Response` headers (`resp.headers().append(...)`). Update the TS client to request it, or
  amend the spec to make the subprotocol optional and record why.

### 5.2 [HIGH] Endpoint paths hardcoded as string literals; no shared constants; incompatible with the server's configurable `path`
- **File:line:** `crates/shamir-transport-ws/src/server.rs:90` (`!= "/shamir/v1"`), `:124`
  (`!= "/shamir/v1/browser"`).
- **Issue:** `/shamir/v1` and `/shamir/v1/browser` are the protocol's version markers on
  the wire, but they exist only as `&'static str` literals inside the two accept functions.
  There is no `pub const` in this crate, and the accept API offers no path parameter.
  Meanwhile the sole production consumer treats the path as operator configuration:
  `shamir-server/src/config.rs:767-777` accepts *any* `path` starting with `/`, then
  `server_launcher.rs:1162/:1266` calls these hardcoded acceptors.
- **Failure scenario:** Operator sets `path: /db-ws` for a ws listener → config boots
  cleanly → every WS upgrade is answered 404 by the hardcoded check → total, silent
  connectivity loss on that listener with no boot-time error. Independently, the literals
  are now duplicated in ≥5 uncoordinated places (shamir-server config/tests,
  `shamir-client-ts` client.ts, deploy `*.ktav` files, docs), so the version string can
  drift between client and server without any compile-time check.
- **Suggested fix:** Export `pub const NATIVE_WS_PATH: &str = "/shamir/v1";` and
  `pub const BROWSER_WS_PATH: &str = "/shamir/v1/browser";` from this crate; either (a)
  make the accept fns take the expected path (or validate server config against the
  constants at boot, refusing unknown paths), or (b) document that the paths are
  protocol-fixed and have the server config validator reject any other value up front
  instead of accepting it.

### 5.3 [MEDIUM] Send path has no frame-size cap and truncates the length prefix at `u32::MAX` — diverges from the TCP sibling *(primary of the send-cap group; also 6.3, 1.4, 3.7)*
- **File:line:** `crates/shamir-transport-ws/src/framing.rs:118` (`let len = payload.len()
  as u32;`), `:114-124`.
- **Issue:** `ws_send` / `ws_send_sink` accept any payload length: no `MAX_WS_FRAME_SIZE`
  check, and `payload.len() as u32` silently wraps for payloads > 4 GiB. The TCP transport
  this crate mirrors does enforce it on send: `shamir-transport-tcp/src/framing.rs:153-156`
  and `:192-195` return `FrameError::TooLarge` for `payload.len() >
  MAX_FRAME_SIZE_DEFAULT`. The asymmetry means the WS send API's contract differs from the
  wire format it claims to share (framing.rs:1-9) — the sender-side error contract is
  strictly weaker than the TCP transport's for the same wire format.
- **Failure scenario:** A caller serializing a large SELECT result (>16 MiB, or merely
  violating the spec §8 cap) gets `Ok(())` from `ws_send_sink`; the frame goes out, and the
  failure surfaces later and remotely — the receiving peer's framing layer returns
  `TooLarge` mid-connection (or buffers it if the peer's `WebSocketConfig` is loose). For a
  >4 GiB payload the prefix wraps, the declared length is corrupt, and the receiver reports
  a baffling `LengthMismatch` on a frame the sender believed valid.
- **Suggested fix:** Mirror the TCP writer: check `payload.len() > MAX_WS_FRAME_SIZE` (or a
  caller-supplied cap symmetric with `ws_recv`'s `max_frame_size` param) and return
  `WsFrameError::TooLarge { actual, max }` before building the buffer —
  `u32::try_from(payload.len()).map_err(|_| WsFrameError::TooLarge { .. })` also removes the
  silent `as u32` truncation path.

### 5.4 [MEDIUM] *(dedup — primary: 3.1)* Unused, version-mismatched direct dependency `tungstenite = "0.29"` while the public API is pinned to tungstenite 0.24
Full write-up at 3.1. This lens's angle: the API-coupling hazard — every `Message` /
`bytes[4..]` site is written against 0.24 semantics with no manifest hint of the coupling,
so a future tokio-tungstenite upgrade that changes `Message::Binary`'s payload type breaks
them all at once.

### 5.5 [MEDIUM] Zero-length frame means "graceful close" on TCP but is a legal empty frame on WS — undocumented divergence in a claimed-identical wire format
- **File:line:** `crates/shamir-transport-ws/src/framing.rs:1-9` (doc claim), `:146-171`
  (recv path); contrast `shamir-transport-tcp/src/framing.rs:8,31` (`length == 0` →
  `FrameError::PeerClose`).
- **Issue:** framing.rs documents "Same wire format as `shamir-transport-tcp::framing`" —
  true for the byte layout (`[u32_be length][payload]`, length excludes prefix, 16 MiB
  cap), but the zero-length semantic differs: TCP defines declared length 0 as a
  graceful-close indicator and surfaces `FrameError::PeerClose`; over WS a 4-byte
  `[0,0,0,0]` message passes the mismatch and cap checks and returns `Ok(())` with an empty
  buffer (close is signaled only by the WS Close frame, framing.rs:173). Nothing in the
  module doc notes the divergence.
- **Failure scenario:** Cross-transport code reuse — a client port that sends the TCP-style
  zero-frame "close" while on WSS gets its message decoded as an empty payload, which then
  fails msgpack deserialization downstream as an opaque protocol error instead of a clean
  close; or a server handler written against TCP semantics treats the empty frame as EOF
  and skips cleanup that the WS path requires.
- **Suggested fix:** Either document the divergence explicitly in the framing.rs header
  ("length 0 is NOT a close indicator here; close is the WS Close frame"), or reject
  declared==0 frames as a protocol error so the two transports stay behaviorally aligned.

### 5.6 [MEDIUM] *(dedup — primary: 1.1 + 1.2)* `accept_browser_ws` — the Origin-enforcing handshake path — has zero integration tests; the framing length-mismatch invariant is also untested
Splits across the two primary entries: the accept-wiring half at 1.1, the framing
error-path half at 1.2. This lens's framing: the subprotocol fix (5.1) edits exactly the
closure whose behavior is untested — landing 5.1 without 1.1's tests re-creates the
silent-regression risk by construction.

### 5.7 [LOW] Exporter-extraction ordering: this crate's doc contradicts its only production caller
- **File:line:** `crates/shamir-transport-ws/src/server.rs:5-7` ("extracts the exporter ...
  AFTER the WS handshake completes") and `:76-77` ("Caller then extracts the TLS
  exporter"); contrast `shamir-server/src/server/server_launcher.rs:1156-1159` ("CRITICAL:
  extract exporter BEFORE the WS upgrade consumes `tls`. After upgrade the TLS state is
  owned by the WebSocketStream and not directly accessible.").
- **Issue:** The public contract in server.rs tells callers to extract the TLS exporter
  *after* `accept_native_ws` returns; the production caller insists it must happen *before*
  the WS upgrade and calls the after-path impossible. Both cannot be right (technically
  `WebSocketStream::get_ref()` may make the after-path workable, but the two crates'
  documentation actively disagree about the safe order for a security-critical step).
- **Failure scenario:** A new integrator (Go/Python client work, second server embedding)
  follows this crate's doc, extracts after the upgrade, and either fails to get the
  exporter or — worse, if it silently returns `None` — falls into a zeros-placeholder
  channel binding without realizing the ordering is contested (see 3.5).
- **Suggested fix:** Settle the ordering once (server_launcher's "before the upgrade" is
  the conservative choice and is what production does), and rewrite server.rs's doc for
  `accept_native_ws` to state it, ideally with the `extract_tls_exporter_from_stream` call
  shown in the doc example.

### 5.8 [LOW] *(dedup — primary: 3.5)* `BROWSER_CHANNEL_BINDING` constant exported but consumers re-hardcode `[0u8; 32]`
Full write-up at 3.5. This lens flagged the native-endpoint fallback (server_launcher.rs:
1159) as the worst instance — the spec-conformant native value is the real exporter, not
the placeholder.

### 5.9 [LOW] *(dedup — primary: 2.2)* `accept_browser_ws` clones the origin policy on every connection
Full write-up at 2.2 (this lens contributed the borrowed-closure alternative: no `'static`
bound on the `Callback` trait → `&'a BrowserOriginPolicy` capture works).

### 5.10 [NIT] *(dedup — primary: 1.5)* Origin matching is case-sensitive; wildcard detection is a whole-pattern substring search
Full write-up at 1.5.

### 5.11 [NIT] *(dedup — primary: 7.6)* `is_loopback` re-implements `IpAddr::is_loopback`
Full write-up at 7.6.

### 5.12 [NIT] *(dedup — primary: 7.5)* Unused dev-dependencies: `hex`, `serde`, `serde_bytes`, `rmp-serde` (plus the direct `tungstenite`, see 3.1)
Full write-up at 7.5. This lens's observation: the unused `rmp-serde` also means the
crate's tests never exercise a real msgpack payload despite the docs calling this
"length-prefix msgpack framing."

## 6. error-handling-lifecycle

Crate-level error discipline is largely faithful to CLAUDE.md: every fallible API returns
`Result` over a `thiserror` enum (`#[from]` where natural), src contains zero
`panic!`/`unwrap`/`anyhow`/`Box<dyn Error>`, and the bind path validates policy *before*
socket creation (nothing to close on the reject path); both accept fns take ownership of
the stream, so any `Err` drops the socket; the crate spawns no tasks, holds no locks, owns
no files — nothing else to leak on an error path. Test layout matches the documented
convention; the covered modules (`browser_tests.rs` 10 tests, `listener_tests.rs`
validate-before-bind) are solid. The real defects sit in the accept layer:

### 6.1 [MEDIUM] `WsAcceptError::OriginRejected` and `WrongPath` are dead variants — all accept rejections surface as `Handshake` *(primary of the dead-variants group; also 1.3)*
- **File:line:** `crates/shamir-transport-ws/src/server.rs:55-71` (enum; `#[from]
  OriginRejected` at :60-61, `WrongPath` at :64-70), `:99` and `:144` (the only `?`
  construction sites); re-exported from `lib.rs:34`.
- **Issue:** Both accept fns implement wrong-path rejection (server.rs:90-94, :124-128) and
  origin rejection (:133-139) by returning `ErrorResponse` from the handshake callback.
  Tungstenite converts that into `Error::Http`, and the single `?` maps it through
  `#[from]` into `WsAcceptError::Handshake`. A workspace-wide grep confirms nothing ever
  constructs `WsAcceptError::OriginRejected(...)` or `WsAcceptError::WrongPath { .. }`;
  the `#[from] OriginRejected` conversion is unreachable. (The `OriginRejected` *type* is
  real — `validate_origin` produces it — but its typed propagation into `WsAcceptError`
  never happens.)
- **Failure scenario:** The first consumer wiring `/shamir/v1/browser` (e.g. shamir-server)
  that matches `WsAcceptError::OriginRejected(_)` to count or escalate policy denials — or
  to map the rejection back to HTTP 403 — never hits that arm; a security-relevant origin
  rejection (the endpoint's primary anti-CSWSH defence) is indistinguishable from a TLS/IO
  handshake failure except by string-matching the `Http` response body (`"origin rejected:
  …"`). Any test asserting `Err(WsAcceptError::OriginRejected(_))` cannot pass by
  construction.
- **Suggested fix:** After the `.await`, inspect the `Error::Http` response (status
  404/403 + reason) and re-map into `WrongPath` / `OriginRejected` before returning, since
  the request is only visible inside the callback. Alternatively remove the dead variants
  and document how to classify from `Handshake(Error::Http)`. Either way, the exported
  error taxonomy should stop promising classification it does not deliver.

### 6.2 [MEDIUM] *(dedup — primary: 1.1 + 1.2)* Error paths of framing and accept have no variant-asserting tests
Splits across 1.1 (accept rejection paths — no test drives either accept fn to an error)
and 1.2 (framing error variants; the only framing error test asserts bare `is_err()`).
This lens's addition: a refactor that wraps peer-close into `Io` or treats `Message::Text`
as skippable passes the whole suite silently — regressions in the fail-closed guarantees
go unnoticed; suggested layout per CLAUDE.md is `src/framing/tests/framing_error_tests.rs`.

### 6.3 [LOW] *(dedup — primary: 5.3)* Send path lacks the TCP sibling's size guard; `payload.len() as u32` truncates silently
Full write-up at 5.3.

### 6.4 [LOW] *(dedup — primary: 3.5)* Exporter extraction failure is indistinguishable from unavailability (`Option` discards the cause)
Full write-up at 3.5. Mitigating note carried from this lens: the wrapper mirrors the
sibling `extract_tls_exporter`, which uses the same `Option` pattern, so the shape is at
least workspace-consistent — minimally, document in caps that `None` on the native
endpoint MUST abort the connection and must never fall back to the placeholder.

### 6.5 [NIT] Framing error leaves the caller's scratch buffer holding the previous frame
- **File:line:** `crates/shamir-transport-ws/src/framing.rs:169-170` (`buf.clear()` only on
  the success path).
- **Issue:** On `LengthMismatch` / `TooLarge` / `PeerClose`, the caller-supplied buffer
  still contains the *previous* frame's payload. (Fine for `ws_recv`'s fresh `Vec`; wrong
  only for reused scratch buffers.)
- **Failure scenario:** A caller that logs `buf` on the error path — or ignores the
  `Result` and proceeds — treats the previous request's bytes as the current frame's.
- **Suggested fix:** Either document that `buf` is unspecified on `Err`, or clear it at
  function entry so the error path is side-effect-free.

### 6.6 [NIT] *(dedup — primary: 4.4)* Doc attributes the 4 KiB pre-auth check to `ws_recv_into`, which enforces nothing by itself
Full write-up at 4.4.

### 6.7 [NIT] `Origin` header containing obs-text bytes is reported as `Missing` rather than rejected-as-present
- **File:line:** `crates/shamir-transport-ws/src/server.rs:129-132` (with `browser.rs:99`).
- **Issue:** `to_str().ok()` maps a present-but-non-ASCII `Origin` header to `None`, so the
  rejection reason becomes `OriginRejected::Missing` ("browser endpoint requires Origin
  header"). The upgrade is still rejected (fail-closed either way), but the error label
  misstates reality, which matters once rejections are logged or matched on.
- **Suggested fix:** Read the raw `HeaderValue`; if `to_str()` fails, return
  `OriginRejected::NotAllowed` (e.g. with a `<non-utf8>` placeholder) so present-but-invalid
  is distinguishable from absent.

## 7. style-claude-md

**Largely exemplary.** Verified against CLAUDE.md: `lib.rs` is module decls + re-exports +
`#[cfg(test)] mod tests;` only; `src/tests/mod.rs` is a manifest-only re-export file; tests
split by topic (`browser_tests.rs`, `listener_tests.rs`, `server_tests.rs`); no
implementation file carries an inline `#[cfg(test)]` block; all `src/` files hoist imports
to the header (grep: zero indented `use` statements crate-wide); inline
`#[allow(clippy::result_large_err)]` at server.rs:87/:122 each carry the required
one-line justification; all error enums derive `thiserror` (no `anyhow`, no leaked
`Box<dyn Error>`, no `panic!` outside tests); one-file-one-closely-coupled-group holds for
every module; the crate-root `tests/framing_round_trip.rs` is a genuine public-API
integration test mirroring the established sibling pattern
(`shamir-transport-tcp/tests/framing.rs`). Coverage claim: 10 origin-policy tests, 9
listener-profile tests, 3 accept-path tests (incl. live 16 MiB cap), 7 framing tests.

### 7.1 [MEDIUM] Mid-file `use` statement violates the "imports at the top" rule
- **File:line:** `crates/shamir-transport-ws/tests/framing_round_trip.rs:98` (spot-checked:
  present, after the section banner at :94-96).
- **Issue:** `use futures_util::{SinkExt, StreamExt};` sits at line 98, after four complete
  test functions and a section-banner comment, instead of in the file header alongside the
  other imports (lines 3-8). CLAUDE.md is explicit: "All `use` statements live in the
  **file header** (or the enclosing module's header), never inside a function or block
  body." This is module-level rather than function-body scope, but it is plainly not the
  file header — the exact drift the rule targets. `SinkExt` (line 131) and `StreamExt`
  (line 108) only resolve because of this buried import.
- **Failure scenario:** A reader or automated tool scanning the header concludes
  `futures_util` is unused; a later edit that reorders or deletes the "Split-half tests"
  banner section silently breaks the two split-half tests below it. Every other file in the
  crate (src and tests) follows the header convention, making this file the outlier.
- **Suggested fix:** Hoist the import into the header block next to lines 3-8 and delete
  line 98. The section banner comment can stay. (The workspace roadmap already schedules an
  imports-at-top sweep touching this crate — fold it in there.)

### 7.2 [LOW] `lib.rs` re-export set incomplete vs. module public APIs (and vs. sibling transport crate) — *partially corrected on spot-check*
- **File:line:** `crates/shamir-transport-ws/src/lib.rs:29-35`; genuine gap at
  `src/framing.rs:26` (`MAX_WS_FRAME_SIZE`).
- **Issue:** The crate's convention is "every public item re-exported at the root", and the
  workspace sibling `shamir-transport-tcp/src/lib.rs:12` re-exports its full framing
  surface including the size const (`read_frame, write_frame, FrameError,
  MAX_FRAME_SIZE_DEFAULT`). **Spot-check correction:** the source file claimed `ws_recv`
  is also missing — it is not; current `lib.rs:30-32` re-exports
  `ws_recv, ws_recv_into, ws_recv_into_stream, ws_send, ws_send_sink, WsFrameError` in
  full (the working tree is clean, so this is a source-file inaccuracy, not drift since
  review). The surviving gap is `MAX_WS_FRAME_SIZE` only.
- **Failure scenario:** Callers who use the root re-exports discover the size const only
  by browsing `framing::`; a hard-coded 16 MiB literal in a caller can silently diverge
  from the const.
- **Suggested fix:** Add `MAX_WS_FRAME_SIZE` to the framing re-export set (or, if the const
  is intentionally kept namespaced, document the intent).

### 7.3 [LOW] *(dedup — primary: 3.5)* `BROWSER_CHANNEL_BINDING` is dead public API; the zero placeholder exists only in prose elsewhere
Full write-up at 3.5. This lens's structural note: it makes `tls_exporter.rs` the one file
whose second export is unanchored to any consumer, and the prose in `lib.rs:11-12` /
`server.rs:11` can drift from the unused const independently.

### 7.4 [LOW] *(dedup — primary: 3.1)* Unused direct dependency `tungstenite = "0.29"` (version-mismatched with tokio-tungstenite)
Full write-up at 3.1 (five-lens group; style angle: lockfile noise + the confusing
trait-bound errors a future direct import would produce).

### 7.5 [LOW] Unused `[dev-dependencies]`: `hex`, `serde`, `serde_bytes`, `rmp-serde` *(primary of the unused-dev-deps group; also 5.12)*
- **File:line:** `crates/shamir-transport-ws/Cargo.toml:32-36`.
- **Issue:** None of `hex`, `serde`, `serde_bytes`, or `rmp-serde` appears in any `.rs`
  file in the crate (grep over `crates/shamir-transport-ws/**/*.rs`: zero matches). All
  test files (`src/tests/*`, `tests/framing_round_trip.rs`) depend only on `tokio`,
  `futures-util`, `tokio-tungstenite`, and the crate itself. Likely leftovers from a test
  that encoded msgpack payloads by hand.
- **Failure scenario:** None at runtime; cost is lockfile/build-graph noise and a false
  signal that framing tests do msgpack round-trips (they don't — payloads are opaque
  bytes).
- **Suggested fix:** Remove the whole `[dev-dependencies]` block (and the direct
  `tungstenite` per 3.1), or keep only entries a future test actually needs — if
  payload-level round-trips are wanted, a single rmp-serde test through `ws_send`/`ws_recv`
  would justify keeping it and simultaneously cover the real payload shape.

### 7.6 [NIT] Redundant `is_loopback` helper duplicates `IpAddr::is_loopback` *(primary of the is_loopback group; also 5.11)*
- **File:line:** `crates/shamir-transport-ws/src/listener.rs:47-52` (spot-checked: present).
- **Issue:** The private `fn is_loopback(ip: IpAddr) -> bool` hand-dispatches to
  `V4::is_loopback`/`V6::is_loopback`, but `IpAddr::is_loopback()` is a stable inherent
  method that already does exactly this — and the crate itself calls it directly at
  `src/tests/listener_tests.rs:62`. The helper adds an indirection layer (and an extra
  private item in a file otherwise free of them) for no behavioural difference.
- **Suggested fix:** Inline to `WsListenerProfile::PlainWsLoopback => addr.ip().is_loopback()`
  at listener.rs:42 and delete the helper.

---

## Finding counts

Raw lens-tagged total (each finding counted as severity-tagged in its own lens file):
**49** — matches the workspace SUMMARY's per-crate breakdown row for this crate exactly.

| Severity | Lens-tagged findings | Finding numbers (dedup groups in one row count once) |
|---|---|---|
| critical | 0 | — |
| high | 3 | 5.1 (subprotocol unimplemented) · 5.2 (hardcoded endpoint paths) · 1.1 + 3.2\* (Origin live-wiring tests — one defect, two lenses) |
| medium | 11 | 1.2 + 5.6 + 6.2 (framing/accept error-path tests — one defect, three lenses) · 3.1 + 5.4 + 4.3\* + 1.6\* + 7.4\* (phantom `tungstenite 0.29` — one defect, five lenses) · 5.3 + 6.3\* + 1.4\* + 3.7\* (send-side cap / `u32` truncation — one defect, four lenses) · 5.5 (zero-length-frame divergence) · 6.1 + 1.3\* (dead `WsAcceptError` variants — one defect, two lenses) · 4.1 + 2.1\* (send alloc / prereserved path defeated — one defect, two lenses) · 7.1 (mid-file `use`) |
| low | 21 | 1.5 + 3.8\* + 5.10\* (origin-matcher robustness — one defect, three lenses) · 3.4 + 4.2 (control-frame flood / uncapped pong queue — one defect, two lenses) · 3.5 + 6.4 + 5.8 + 1.8\* + 7.3\* (exporter placeholder story — one defect, five lenses) · 2.2 + 5.9 + 4.5\* (policy deep-clone per accept — one defect, three lenses) · 4.4 + 3.6\* + 6.6\* + 1.9\* (pre-auth-cap doc misattribution + companion doc warts — one defect, four lenses) · 5.7 (exporter ordering doc contradiction) · 3.3 (Origin echoed into 403 body) · 7.2 (lib.rs re-export set, corrected) · 7.5 + 5.12\* (unused dev-deps — one defect, two lenses) |
| nit | 14 | 1.7 (phantom `accept_no_origin` doc) · 6.5 (scratch buffer unspecified on `Err`) · 6.7 (obs-text Origin reported `Missing`) · 7.6 + 5.11 (redundant `is_loopback` helper — one defect, two lenses) |
| **total** | **49** | lens-tagged findings; **23 distinct defects** after dedup |

\* tagged lower in its own lens file; the dedup group is listed under its highest tag, and
every member is still counted in its own severity row above — columns therefore sum to 49.

Deduplicated defect census: **0 critical, 3 high, 7 medium, 9 low, 4 nit = 23 distinct
defects** (49 lens-tagged findings across the seven files; the phantom-dependency and
exporter-placeholder defects were each independently flagged by five of the seven lenses).

## Fix Plan

**P0 — before anything else ships from this crate**

1. **Resolve hardcoded paths vs configurable `path` (5.2).** Export
   `NATIVE_WS_PATH`/`BROWSER_WS_PATH` consts; either parameterize the accept fns or make
   the server config validator reject/refuse any other ws path **at boot** with a clear
   error. Closes: 5.2 (silent total connectivity loss on reconfigured listeners).
2. **Implement or formally drop the `shamir-v1` subprotocol (5.1).** Negotiate
   `Sec-WebSocket-Protocol` in both callbacks (reject 400 on mismatch, echo `shamir-v1`),
   and update `shamir-client-ts` to request it — or amend the spec and record why. Red
   test: conformant handshake must succeed. Closes: 5.1.
3. **Live handshake tests for the Origin control, Red first per CLAUDE.md TDD (1.1/3.2).**
   Allowed/missing/disallowed Origin → 101/403/403; wrong path → 404; non-UTF-8 Origin →
   rejected; plus the framing error-variant tests and the tightened
   `rejects_oversized_frame` assertion (1.2/5.6/6.2). Closes: 1.1, 1.2, 3.2, 5.6, 6.2 —
   the untested-security-control cluster.
4. **Delete the phantom `tungstenite = "0.29"` dependency (and the unused direct
   `rustls`/`tokio-rustls` entries while there) (3.1).** One-line manifest fix; removes the
   CVE-false-assurance trap and the second compiled parser. Closes: 3.1, 5.4, 4.3, 1.6,
   7.4.

**P1 — soon**

5. **Send-side frame-size guard (5.3):** reject `payload.len() > MAX_WS_FRAME_SIZE` (or a
   symmetric caller-supplied cap) via `u32::try_from` before the cast. Closes: 5.3, 6.3,
   1.4, 3.7.
6. **Make the accept error taxonomy real (6.1):** inspect `Error::Http` status after the
   handshake and re-map into `WrongPath` (404) / `OriginRejected` (403), or delete the dead
   variants and document classification from `Handshake`. Closes: 6.1, 1.3.
7. **Zero-alloc WS send variant (4.1):** `ws_send_sink_vec(sink, Vec<u8>)` ownership-taking
   variant; override `write_frame_prereserved` in shamir-server's `WsFrameWriter`; document
   copy semantics. Closes: 4.1, 2.1.
8. **Control-frame budget (3.4):** cap consecutive non-BINARY messages and return
   `WsFrameError::ControlFrameFlood`; verify/enforce a write-buffer cap for
   tokio-tungstenite 0.24 (or plan the 0.26+ `write_buffer_size` bump). Closes: 3.4, 4.2.
9. **Fail-closed exporter story (3.5):** `channel_binding_for(stream, BindingMode) ->
   Result<[u8; 32], ChannelBindingError>` (error on native-path extraction failure,
   placeholder only for TlsNoExport); wire `BROWSER_CHANNEL_BINDING` into both
   `server_launcher` sites or rename/gate it. Closes: 3.5, 6.4, 5.8, 1.8, 7.3.
10. **Settle the exporter-extraction ordering doc (5.7):** server.rs must agree with
    server_launcher's extract-before-upgrade contract. Closes: 5.7.
11. **Zero-length-frame alignment decision (5.5):** minimum — document the divergence in
    framing.rs's header; better — reject declared==0 so both transports behave identically.
    Closes: 5.5.

**P2 — backlog**

12. **Origin-matcher hardening (1.5):** `try_allow -> Result` (reject scheme-less/malformed
    patterns at boot), reject `*`-leading labels, ASCII-lowercase comparisons, anchor
    wildcard detection. Closes: 1.5, 3.8, 5.10.
13. **Stop deep-cloning the origin policy per accept (2.2):** `Arc<BrowserOriginPolicy>`
    (or borrowed-closure) signature; mechanical switch of the `server_launcher` field.
    Closes: 2.2, 5.9, 4.5.
14. **Static 403 body (3.3):** stop reflecting the attacker-controlled Origin into the
    response body; keep it in structured `tracing` fields. Closes: 3.3.
15. **Doc hygiene pass (4.4 + 1.7 + 6.5 + 6.7 + 7.2):** reword the 4-KiB attribution at
    server.rs:26-28 (and server_tests.rs:11-13) + `LengthMismatch::actual` field doc;
    delete the phantom `accept_no_origin` sentence; document `buf`-on-`Err` (or clear at
    entry); classify non-UTF-8 Origin as present-but-invalid; add `MAX_WS_FRAME_SIZE` to
    the root re-exports. Closes: 4.4, 3.6, 6.6, 1.9, 1.7, 6.5, 6.7, 7.2.
16. **Style/backlog sweep:** hoist the mid-file `use` (7.1 — fold into the workspace
    imports-at-top sweep); delete the four unused dev-deps (7.5, 5.12) or add one real
    msgpack round-trip test; inline `is_loopback` (7.6, 5.11).

</details>
