<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-transport-tcp — error-handling-lifecycle revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

Misclassified framing errors, boxed TLS errors and missing stream contracts remain. Provider panic risk is conditional rather than caused by missing explicit installation alone.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 9 | 9 | 0 | 0 | 0 | 0 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — TLS config builders can panic on missing/ambiguous rustls crypto provider

Status: `confirmed-open`. Current risk: `medium`.

Builders still select ambient providers through a panicking convenience API. Pinned rustls auto-installs a unique built-in provider; panic remains possible with ambiguous/custom-provider feature combinations or a provider incompatible with TLS 1.3. Current AWS-LC defaults are not evidence of a live boot panic.

Evidence: [crates/shamir-transport-tcp/src/tls.rs:54](../../../../../crates/shamir-transport-tcp/src/tls.rs#L54); [crates/shamir-transport-tcp/src/tls.rs:64](../../../../../crates/shamir-transport-tcp/src/tls.rs#L64); [crates/shamir-transport-tcp/Cargo.toml:18](../../../../../crates/shamir-transport-tcp/Cargo.toml#L18); [Cargo.lock:3048](../../../../../Cargo.lock#L3048).

Grouping/duplicate: `SUMMARY.md#6.3`. This row is not another independent defect.

<a id="review-2"></a>

### Claim 2 — `Box<dyn Error + Send + Sync>` in library APIs instead of a `thiserror` enum

Status: `confirmed-open`. Current risk: `medium`.

TLS constructors retain boxed and string-based errors. Server lifecycle code still flattens them into TlsError::Build(String), although boxing does not technically force callers to stringify concrete errors.

Evidence: [crates/shamir-transport-tcp/src/tls.rs:30](../../../../../crates/shamir-transport-tcp/src/tls.rs#L30); [crates/shamir-transport-tcp/src/tls.rs:44](../../../../../crates/shamir-transport-tcp/src/tls.rs#L44); [crates/shamir-transport-tcp/src/tls.rs:50](../../../../../crates/shamir-transport-tcp/src/tls.rs#L50); [crates/shamir-server/src/tls.rs:92](../../../../../crates/shamir-server/src/tls.rs#L92); [crates/shamir-server/src/tls.rs:116](../../../../../crates/shamir-server/src/tls.rs#L116).

Grouping/duplicate: `SUMMARY.md#6.2`. This row is not another independent defect.

<a id="review-3"></a>

### Claim 3 — `write_frame_prereserved` reports every contract violation as `TooLarge`

Status: `confirmed-open`. Current risk: `medium`.

Both malformed argument branches still produce TooLarge and tests assert it. Actual oversized payload rejection remains valid; malformed cases lack their own error classification.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:241](../../../../../crates/shamir-transport-tcp/src/framing.rs#L241); [crates/shamir-transport-tcp/src/framing.rs:250](../../../../../crates/shamir-transport-tcp/src/framing.rs#L250); [crates/shamir-transport-tcp/tests/framing.rs:309](../../../../../crates/shamir-transport-tcp/tests/framing.rs#L309); [crates/shamir-server/src/framer.rs:317](../../../../../crates/shamir-server/src/framer.rs#L317).

Grouping/duplicate: `SUMMARY.md#6.1`. This row is not another independent defect.

<a id="review-4"></a>

### Claim 4 — Missing error-path tests: TLS config builders and write-side size rejection

Status: `confirmed-open`. Current risk: `low`.

No registered transport test pins TLS parse/key/mismatch errors or oversized writer rejection. Generation is exercised successfully, not through a failure oracle.

Evidence: [crates/shamir-transport-tcp/src/tests/mod.rs:1](../../../../../crates/shamir-transport-tcp/src/tests/mod.rs#L1); [crates/shamir-transport-tcp/tests/handshake_e2e.rs:131](../../../../../crates/shamir-transport-tcp/tests/handshake_e2e.rs#L131); [crates/shamir-transport-tcp/tests/framing.rs:309](../../../../../crates/shamir-transport-tcp/tests/framing.rs#L309).

Grouping/duplicate: `SUMMARY.md#1.2, SUMMARY.md#1.4`. This row is not another independent defect.

<a id="review-5"></a>

### Claim 5 — Post-error stream state is undocumented (`TooLarge` desyncs the stream)

Status: `confirmed-open`. Current risk: `low`.

Read documentation still omits recovery obligations. TooLarge consumes the header without its payload; I/O errors may leave partial reads. The server correctly tears down on read error.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:36](../../../../../crates/shamir-transport-tcp/src/framing.rs#L36); [crates/shamir-transport-tcp/src/framing.rs:56](../../../../../crates/shamir-transport-tcp/src/framing.rs#L56); [crates/shamir-transport-tcp/src/framing.rs:72](../../../../../crates/shamir-transport-tcp/src/framing.rs#L72); [crates/shamir-server/src/connection/request_loop.rs:283](../../../../../crates/shamir-server/src/connection/request_loop.rs#L283).

Grouping/duplicate: `SUMMARY.md#6.4`. This row is not another independent defect.

<a id="review-6"></a>

### Claim 6 — Cancel-safety contract lives in the consumer, not the crate

Status: `confirmed-open`. Current risk: `low`.

Neither reader documents that cancellation can consume part of a frame and invalidate continuation. The server's local cancellation explanation remains, and it discards the connection rather than resuming the read.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:45](../../../../../crates/shamir-transport-tcp/src/framing.rs#L45); [crates/shamir-transport-tcp/src/framing.rs:98](../../../../../crates/shamir-transport-tcp/src/framing.rs#L98); [crates/shamir-server/src/connection/request_loop.rs:231](../../../../../crates/shamir-server/src/connection/request_loop.rs#L231); [crates/shamir-server/src/connection/request_loop.rs:279](../../../../../crates/shamir-server/src/connection/request_loop.rs#L279).

Grouping/duplicate: `SUMMARY.md#6.5`. This row is not another independent defect.

<a id="review-7"></a>

### Claim 7 — `extract_tls_exporter` collapses distinct failures into `None`

Status: `confirmed-open`. Current risk: `low`.

The helper still uses .ok()? and cannot expose HandshakeNotComplete or another rustls error to callers. Production client handling is an error return, not a panic.

Evidence: [crates/shamir-transport-tcp/src/tls.rs:81](../../../../../crates/shamir-transport-tcp/src/tls.rs#L81); [crates/shamir-client/src/client.rs:480](../../../../../crates/shamir-client/src/client.rs#L480).

Grouping/duplicate: `SUMMARY.md#3.3`. This row is not another independent defect.

<a id="review-8"></a>

### Claim 8 — nit — SAFETY argument in `read_frame_into` rests on callee behavior

Status: `confirmed-open`. Current risk: `high`.

This is part of the high-severity soundness defect, not merely a comment nit: ReadBuf::new treats the entire destination as initialized, allowing a legal AsyncRead implementation to observe it. Error cleanup cannot establish the required initialization precondition.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:118](../../../../../crates/shamir-transport-tcp/src/framing.rs#L118); [crates/shamir-transport-tcp/src/framing.rs:127](../../../../../crates/shamir-transport-tcp/src/framing.rs#L127); [crates/shamir-transport-tcp/src/framing.rs:129](../../../../../crates/shamir-transport-tcp/src/framing.rs#L129); [Cargo.lock:4195](../../../../../Cargo.lock#L4195).

Grouping/duplicate: `SUMMARY.md#3.1`. This row is not another independent defect.

<a id="review-9"></a>

### Claim 9 — nit — write-side size check hardcodes `MAX_FRAME_SIZE_DEFAULT`

Status: `confirmed-open`. Current risk: `nit`.

All writers still use the default constant while readers take a limit parameter; no writer phase-limit abstraction has been added.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:153](../../../../../crates/shamir-transport-tcp/src/framing.rs#L153); [crates/shamir-transport-tcp/src/framing.rs:192](../../../../../crates/shamir-transport-tcp/src/framing.rs#L192); [crates/shamir-transport-tcp/src/framing.rs:250](../../../../../crates/shamir-transport-tcp/src/framing.rs#L250).

Grouping/duplicate: `SUMMARY.md#5.1`. This row is not another independent defect.

## Corrections and qualified non-findings

- Provider preinstallation in tests does not prove it is required: pinned rustls can auto-install AWS-LC. Preserve only the conditional ambiguity/custom/incompatible-provider panic claim.
- The error-path clear is present but does not make the preceding initialized-buffer exposure sound or handle cancellation.
- After an error a stream may be partially consumed, not necessarily desynchronized in every possible case. Document a conservative discard requirement without the universal factual assertion.
- The report's oversized static-slice suggestion avoids a heap allocation only, not the 16 MiB storage footprint.
- Boxed errors can be downcast; the server's string flattening is an existing caller decision, not an unavoidable property of boxing.
- Tests named Miri-safe are registered integration tests, but neither Miri success nor current execution was verified.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-transport-tcp -- Error handling & resource lifecycle

## Summary

The crate largely honors the documented discipline: `FrameError` / `ListenerBindError` are proper `thiserror` enums, `?` is used throughout, bind validation fails closed *before* the socket exists, and the framing error paths are unusually well tested (including a Miri-run test that pins the `buf.clear()` cleanup on the unsafe `set_len` read path). The weak spots concentrate in `tls.rs`: two library APIs return `Box<dyn Error + Send + Sync>` (explicitly a last resort per CLAUDE.md) forcing the only server consumer to stringify errors, and the rustls config builders carry latent panic paths that the crate's own tests must paper over by pre-installing a crypto provider. The remainder is contract hygiene: a misleading reuse of `FrameError::TooLarge` for caller-contract violations, undocumented post-error stream state, and a handful of error paths with no tests.

## Findings

### 1. TLS config builders can panic on missing/ambiguous rustls crypto provider
- **File:line:** `src/tls.rs:54` (`make_server_config_from_pem`), `src/tls.rs:63-69` (`make_client_config_no_ca`)
- **Severity:** medium
- **Issue:** `ServerConfig::builder_with_protocol_versions` / `ClientConfig::builder_with_protocol_versions` resolve the *process-default* `CryptoProvider` and **panic** when none is installed and the unified crate features are ambiguous (both `ring` and `aws_lc_rs` present anywhere in the feature graph). `make_client_config_no_ca` returns `Arc<ClientConfig>` directly — not fallible — so this failure mode is unrepresentable in its signature. Every test in the crate must pre-install a provider (`let _ = default_provider().install_default();` in `tests/handshake_e2e.rs:116`, `tests/tls13_only.rs:19-21`, `tests/echo_e2e.rs:154`), which is evidence the library depends on ambient process state it neither sets nor checks. This violates CLAUDE.md's "Return `Result<T, E>`. Avoid `panic!`" pillar.
- **Failure scenario:** a future workspace dependency enables rustls's `ring` feature; feature unification makes the provider ambiguous; `make_server_config_from_pem` then panics at server boot (`shamir-server/src/tls.rs::load_or_generate` installs no provider) instead of returning an `Err` the boot path could report.
- **Suggested fix:** thread an explicit provider: `ClientConfig::builder_with_provider(provider).with_protocol_versions(&[&rustls::version::TLS13])` (returns `Result`), or at minimum fetch `CryptoProvider::get_default()` and return a typed error when absent. Make both functions return `Result<_, TlsConfigError>` (see finding 2).

### 2. `Box<dyn Error + Send + Sync>` in library APIs instead of a `thiserror` enum
- **File:line:** `src/tls.rs:30` (`generate_self_signed_server_cert`), `src/tls.rs:44` (`make_server_config_from_pem`), plus the ad-hoc stringly error at `src/tls.rs:50` (`ok_or("no PKCS8 key in PEM")??`)
- **Severity:** medium
- **Issue:** CLAUDE.md: "`thiserror` for library error enums (with `#[from]` where natural)"; "`Box<dyn Error>` is a last resort for boundary code." These are the crate's public, boot-critical APIs. The cost is already visible downstream: `shamir-server/src/tls.rs:92,116` must stringify (`TlsError::Build(e.to_string())`), collapsing rcgen generation failures, PEM-parse failures, and rustls config-build failures into one opaque `String`; no caller can `match` on the cause (e.g. to distinguish "corrupt key file" from "malformed cert" in ops alerting).
- **Failure scenario:** operator misconfigures or a disk fault truncates a PEM; the server logs `tls build: <flat string>` and there is no programmatic way for the launcher/tests to branch on the failure class.
- **Suggested fix:** `#[derive(Debug, Error)] pub enum TlsConfigError { #[from] Rcgen(rcgen::Error), #[from] Pem(rustls_pki_types::pem::Error), #[from] Rustls(rustls::Error), #[error("no PKCS8 key in PEM")] NoKeyInPem }` and return it from both functions.

### 3. `write_frame_prereserved` reports every contract violation as `TooLarge`
- **File:line:** `src/framing.rs:241-255`
- **Severity:** medium
- **Issue:** `buf.len() < 4` yields `FrameError::TooLarge { actual: 0, max: 16 MiB }` — the `Display` output ("frame too large: 0 > 16777216") is simply false. A prefix/payload mismatch (`declared_len != actual_payload`) also yields `TooLarge` even when both numbers are far below the cap, so the rendered error names the wrong failure ("frame too large: 999 > 16777216" when the real problem is 999 ≠ 100). `map_tcp_err` (`shamir-server/src/framer.rs:313-320`) forwards the variant verbatim, so an internal producer bug surfaces to operators as a genuine peer size-limit violation. `tests/framing.rs:309-324` pins this behavior — it pins the wrong contract.
- **Failure scenario:** a serialization change writes a wrong prefix; dashboards/log-based `TooLarge` rate alerts fire and the investigation chases a phantom malicious/oversized peer instead of the local bug.
- **Suggested fix:** dedicated variants for caller-contract violations, e.g. `PrefixTooShort(usize)` and `PrefixMismatch { declared: usize, actual: usize }`. These are programmer bugs, a different error class from the protocol-limit rejection; keep `TooLarge` exclusively for size-cap violations.

### 4. Missing error-path tests: TLS config builders and write-side size rejection
- **File:line:** `tests/` (absent coverage); affected code `src/tls.rs:41-58`, `src/framing.rs:153-158`, `src/framing.rs:192-197`
- **Severity:** low
- **Issue:** the theme's core paths with zero coverage: (a) `make_server_config_from_pem` with garbage/empty PEM, cert-only PEM (the `NoKeyInPem` arm at `tls.rs:50`), or a mismatched key — never tested (`tls13_only.rs` only exercises negotiation refusal); (b) the write-side `TooLarge` checks in `write_frame` / `write_frame_into` (`framing.rs:153-158`, `192-197`) — the read side has `rejects_oversized_frame_declaration`, the write side has nothing; (c) `generate_self_signed_server_cert` is only ever called on the happy path.
- **Failure scenario:** a refactor of the PEM-parsing or size-check arms regresses silently; TDD protocol (CLAUDE.md) expects the error paths to have a red/green pair like everything else.
- **Suggested fix:** a small `tests/tls_config_errors.rs` asserting `Err` per failure class (which finding 2's enum makes assertable by variant), plus one oversized-write test — a `&[0u8; N]` slice over `MAX + 1` needs no 16 MiB allocation if written against `write_frame_into` with a static slice.

### 5. Post-error stream state is undocumented (`TooLarge` desyncs the stream)
- **File:line:** `src/framing.rs:45-70` (`read_frame`), `src/framing.rs:98-138` (`read_frame_into`)
- **Severity:** low
- **Issue:** after `TooLarge`, the 4-byte prefix is consumed but the peer's payload bytes are still in flight; after *any* `Err` the stream is mid-frame. The public docs say nothing about this. The production caller happens to do the right thing (`shamir-server/src/connection/request_loop.rs:283-287` breaks on any `Err` and tears down), but that invariant lives in the consumer, not in the crate that owns the contract. (`PeerClose` is the one benign case — nothing follows a length-zero frame.)
- **Failure scenario:** a new caller matches `TooLarge` to send an error envelope and keeps reading the same stream; the abandoned payload bytes are parsed as the next length prefix, producing garbage frames or a cascade of false `TooLarge`/`PeerClose` errors.
- **Suggested fix:** document on both read functions: "On any `Err` other than `PeerClose` the stream is left desynchronized — the caller MUST drop the connection."

### 6. Cancel-safety contract lives in the consumer, not the crate
- **File:line:** `src/framing.rs:45-138`
- **Severity:** low
- **Issue:** tokio's `read_exact` is not cancel-safe: dropping a `read_frame(_into)` future mid-frame silently consumes N bytes and desyncs the stream *without any error surfacing*. `request_loop.rs:231-236` (and 276-277) documents locally why its `select!` is safe ("Cancel-safety of the read branch is intentionally NOT required"); the knowledge exists only in that consumer.
- **Failure scenario:** a future caller wraps these functions in `tokio::time::timeout` (a natural per-read slow-loris defense) and continues reading after the timeout — frames silently split, payloads corrupt, with no error to catch.
- **Suggested fix:** add a `# Cancel-safety` doc section to `read_frame`/`read_frame_into`: the future must run to completion, or the stream must be discarded.

### 7. `extract_tls_exporter` collapses distinct failures into `None`
- **File:line:** `src/tls.rs:77-83`
- **Severity:** low
- **Issue:** `.ok()?` merges "handshake not yet complete" and "exporter extraction failed" into a single `None`, and discards the `rustls::Error` reason entirely. The doc comment even acknowledges two different causes. This is against CLAUDE.md's `?`-propagation discipline: the caller cannot distinguish a retryable not-ready state from a broken TLS session, and the diagnostic is unrecoverable.
- **Failure scenario:** channel binding fails intermittently in production; the logs show only a `None`/`expect("exporter")` panic downstream (`tests/handshake_e2e.rs:147` models this) with no rustls error to explain why.
- **Suggested fix:** return `Result<Option<[u8; 32]>, rustls::Error>` (`None` = handshake pending) or a one-variant `thiserror` wrapper that carries the rustls error.

### 8. nit — SAFETY argument in `read_frame_into` rests on callee behavior
- **File:line:** `src/framing.rs:118-128`
- **Severity:** nit
- **Issue:** the *error-path* cleanup itself is done right (`buf.clear()` on read error; pinned by the Miri test at `tests/framing.rs:226-250`). But the claim "the uninit bytes are never observed by safe code" also relies on the `AsyncRead` implementation never *reading* the output slice — the `poll_read` contract does not guarantee that; it only guarantees the callee's writes are visible. Strictly sound form is reading into `&mut [MaybeUninit<u8>]` (e.g. `bytes::BytesMut` + `read_buf`). Flagging as a nit; this lane arguably belongs to a soundness-focused reviewer.

### 9. nit — write-side size check hardcodes `MAX_FRAME_SIZE_DEFAULT`
- **File:line:** `src/framing.rs:153`, `src/framing.rs:192`, `src/framing.rs:250`
- **Severity:** nit
- **Issue:** reads take `max_frame_size` as a parameter; all three writers hardcode the default constant. If a deployment ever negotiates a different cap (the read side is parameterized for exactly that), the writer rejects legal frames or validates against a stale constant. The error-detection policy is asymmetric across the two directions of the same wire format.
- **Suggested fix:** accept a `max_frame_size` parameter (or a shared `FrameLimits` struct) mirroring the read side.

</details>
