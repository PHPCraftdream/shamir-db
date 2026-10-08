<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-transport-tcp — security-crypto revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

The unsafe pooled-read defect remains high priority. TLS 1.3 restriction, protocol-layer authentication and returned PEM zeroization are source-supported, but the public verifier still skips CertificateVerify and exporter diagnostics remain erased.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 5 | 5 | 0 | 0 | 0 | 0 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — `read_frame_into`: unsafe `set_len` before `.await` — formal UB on `&mut [u8]` over uninit memory, and cancellation leaves `buf.len()` covering uninitialized bytes

Status: `confirmed-open`. Current risk: `high`.

Fresh or grown capacity is exposed as initialized before read_exact, whose pinned Tokio implementation constructs ReadBuf::new. Cancellation skips buf.clear. Production timeout/select paths reach this code but discard cancelled buffers; remote disclosure is not established.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:124](../../../../../crates/shamir-transport-tcp/src/framing.rs#L124); [crates/shamir-transport-tcp/src/framing.rs:129](../../../../../crates/shamir-transport-tcp/src/framing.rs#L129); [crates/shamir-transport-tcp/src/framing.rs:134](../../../../../crates/shamir-transport-tcp/src/framing.rs#L134); [crates/shamir-server/src/framer.rs:224](../../../../../crates/shamir-server/src/framer.rs#L224); [crates/shamir-server/src/connection/handshake.rs:717](../../../../../crates/shamir-server/src/connection/handshake.rs#L717); [crates/shamir-server/src/connection/request_loop.rs:280](../../../../../crates/shamir-server/src/connection/request_loop.rs#L280); [Cargo.lock:4195](../../../../../Cargo.lock#L4195).

Grouping/duplicate: `SUMMARY.md#3.1`. This row is not another independent defect.

<a id="review-2"></a>

### Claim 2 — `NoCaVerify` disables all server authentication — chain, CertificateVerify proof-of-possession, and hostname — guarded only by a doc comment

Status: `confirmed-open`. Current risk: `medium`.

Certificate and TLS 1.3 signature callbacks still return unconditional assertions. This public configuration is unauthenticated until protocol checks succeed; the production Rust client does perform SCRAM, identity-pin and exporter-bound signature checks before returning a connection.

Evidence: [crates/shamir-transport-tcp/src/tls.rs:138](../../../../../crates/shamir-transport-tcp/src/tls.rs#L138); [crates/shamir-transport-tcp/src/tls.rs:158](../../../../../crates/shamir-transport-tcp/src/tls.rs#L158); [crates/shamir-client/src/client.rs:484](../../../../../crates/shamir-client/src/client.rs#L484); [crates/shamir-client/src/client.rs:614](../../../../../crates/shamir-client/src/client.rs#L614); [crates/shamir-connect/src/client/handshake.rs:258](../../../../../crates/shamir-connect/src/client/handshake.rs#L258).

Grouping/duplicate: `SUMMARY.md#3.2`. This row is not another independent defect.

<a id="review-3"></a>

### Claim 3 — `extract_tls_exporter` collapses all errors (incl. incomplete handshake) into `None`, inviting constant/absent channel binding

Status: `confirmed-open`. Current risk: `low`.

The helper still discards rustls errors. Contrary to the report, production TCP and native-WS server callers already use a zero fallback; they call after successful TLS handshakes, so a trigger for None with normal pinned TLS 1.3 is not established. The Rust client fails closed.

Evidence: [crates/shamir-transport-tcp/src/tls.rs:77](../../../../../crates/shamir-transport-tcp/src/tls.rs#L77); [crates/shamir-server/src/server/server_launcher.rs:1163](../../../../../crates/shamir-server/src/server/server_launcher.rs#L1163); [crates/shamir-server/src/server/server_launcher.rs:1391](../../../../../crates/shamir-server/src/server/server_launcher.rs#L1391); [crates/shamir-client/src/client.rs:480](../../../../../crates/shamir-client/src/client.rs#L480).

Grouping/duplicate: `SUMMARY.md#3.3`. This row is not another independent defect.

<a id="review-4"></a>

### Claim 4 — Public library API returns `Box<dyn Error + Send + Sync>` instead of a `thiserror` enum

Status: `confirmed-open`. Current risk: `low`.

TLS constructors retain boxed errors and an untyped missing-key error; no TlsConfigError exists.

Evidence: [crates/shamir-transport-tcp/src/tls.rs:30](../../../../../crates/shamir-transport-tcp/src/tls.rs#L30); [crates/shamir-transport-tcp/src/tls.rs:44](../../../../../crates/shamir-transport-tcp/src/tls.rs#L44); [crates/shamir-transport-tcp/src/tls.rs:50](../../../../../crates/shamir-transport-tcp/src/tls.rs#L50).

Grouping/duplicate: `SUMMARY.md#6.2`. This row is not another independent defect.

<a id="review-5"></a>

### Claim 5 — Write-side frame cap is hardcoded, and the too-short-buffer error uses a misleading sentinel

Status: `confirmed-open`. Current risk: `nit`.

Both bundled conditions remain: hardcoded writer limits and TooLarge for malformed prefixes. These are API/diagnostic limitations, not independently demonstrated remote exploits.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:153](../../../../../crates/shamir-transport-tcp/src/framing.rs#L153); [crates/shamir-transport-tcp/src/framing.rs:192](../../../../../crates/shamir-transport-tcp/src/framing.rs#L192); [crates/shamir-transport-tcp/src/framing.rs:241](../../../../../crates/shamir-transport-tcp/src/framing.rs#L241); [crates/shamir-transport-tcp/src/framing.rs:250](../../../../../crates/shamir-transport-tcp/src/framing.rs#L250).

Grouping/duplicate: `SUMMARY.md#5.1, SUMMARY.md#6.1`. This row is not another independent defect.

## Corrections and qualified non-findings

- Replace UB on every call with an unsound API reachable when the accepted payload covers uninitialized capacity. Early close/oversize returns and wholly previously initialized storage are different cases.
- A Drop guard fixes cancellation cleanup only; it does not repair passing uninitialized storage through ReadBuf::new.
- Current cancelled server reads discard their buffers. Potential disclosure through a caller inspecting a cancelled buffer is not a demonstrated remote production exploit.
- Skipping CertificateVerify does not make post-handshake certificate/SPKI pinning impossible; it omits proof-of-possession verification. Restoring that proof does not replace protocol identity authentication.
- Do not claim constant exporter bytes alone permit recorded-proof replay: auth_message also includes fresh client and server nonces; see crates/shamir-connect/src/common/auth_message.rs:87. The relevant weakening is loss of channel separation and relay resistance.
- The 32-byte exporter contract is already documented at crates/shamir-transport-tcp/src/tls.rs:71 and specified at docs/guide-docs/client-server-protocol-spec/TRANSPORT_TCP.md:41.
- Pinned rustls 0.23.37 has no dangerous_configuration feature; the danger API usage is not a missing-feature defect. Negative TLS-version tests are registered but were not executed.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-transport-tcp -- Security & crypto boundary

## Summary

This crate is the TLS/framing boundary, not the SCRAM implementation (HMAC/SCRAM/Argon2 live in `shamir-connect`), and its crypto posture is largely sound: TLS 1.3-only is enforced on both sides with negative tests (`tests/tls13_only.rs`), the exporter is extracted per RFC 9266 and proven equal on both ends in e2e tests, the Plain listener profile fails closed on non-loopback binds (incl. `0.0.0.0`/`::`), and the private-key PEM is `Zeroizing`-wrapped. The dominant theme risk is the crate's one `unsafe` block: `read_frame_into` sets `Vec::len` over uninitialized memory before the `.await`, which is formally UB on every call and leaves a poisoned buffer after cancellation — a pattern the production server actually uses (`select!`/`timeout` around this exact function). Second is the trust boundary: `make_client_config_no_ca()` disables *all* server authentication (chain, CertificateVerify proof-of-possession, hostname) with only a doc comment as the guardrail; correctness depends on every caller performing the Ed25519 pin + exporter-bound signature check at the protocol layer. (Note: the ungated use of `rustls::client::danger` is fine — rustls 0.23.37, as resolved in `Cargo.lock`, no longer has a `dangerous_configuration` feature at all.)

## Findings

### 1. `read_frame_into`: unsafe `set_len` before `.await` — formal UB on `&mut [u8]` over uninit memory, and cancellation leaves `buf.len()` covering uninitialized bytes
- **File:line:** `crates/shamir-transport-tcp/src/framing.rs:117-136` (unsafe at 124-128, await at 129)
- **Severity:** high
- **Issue:** `buf.reserve(len)` + `unsafe { buf.set_len(len) }` exposes uninitialized capacity as `&mut Vec<u8>`, which is then deref-coerced to `&mut [u8]` and passed to `reader.read_exact(buf).await`. (a) Creating a `&mut [u8]` over uninitialized bytes is itself undefined behavior (reference validity requires initialized memory) — tokio's `read_exact` wraps the slice in `ReadBuf::new`, a constructor that asserts initialization. The code even carries `#[allow(clippy::uninit_vec)]`, the lint that exists to flag exactly this. (b) The SAFETY comment only covers the Ok/Err outcomes, but this is an `async fn`: if the future is **dropped mid-await** (cancellation via `select!` or `timeout` — not an Err return), no code runs, and the caller is left holding `buf` with `len == declared_len` while the tail is still uninitialized. Any later read of `buf[..]` is UB and can disclose stale heap bytes. The error-path `buf.clear()` (129-136) does not run on cancellation.
- **Failure scenario:** a caller wraps `read_frame_into` in `tokio::time::timeout(...)` or `select!` (exactly what `shamir-server/src/connection/handshake.rs:716-717` and `request_loop.rs:279-308` do today). Both current call sites happen to discard `frame_buf` after a cancelled branch (and `request_loop.rs` builds a fresh `Vec` per iteration), so nothing *currently* observes the uninit tail — but the crate neither documents this poison-after-cancellation contract nor enforces it; the next caller that logs/inspects the buffer after a timed-out read gets undefined behavior / potential secret disclosure from freed memory. The "Miri-safe" tests (`tests/framing.rs:189-250`) cover only the happy and error paths — no cancellation-path test exists.
- **Suggested fix:** make the uninitialized window unobservable instead of argued away: read into `buf.spare_capacity_mut()` via `ReadBuf::uninit` (small `poll_fn` loop over `AsyncRead::poll_read`), and issue `unsafe { buf.set_len(n) }` only after the read completes with n initialized bytes. Interim hardening: a `Drop` guard holding `&mut Vec<u8>` across the `.await` that `clear()`s on any exit (covers cancellation), a prominent `# Cancellation` section in the doc comment, and a `tokio::time::timeout`-based regression test alongside the Miri pair.

### 2. `NoCaVerify` disables all server authentication — chain, CertificateVerify proof-of-possession, and hostname — guarded only by a doc comment
- **File:line:** `crates/shamir-transport-tcp/src/tls.rs:63-69` (`make_client_config_no_ca`), `124-171` (`NoCaVerify`), esp. `152-159`
- **Severity:** medium
- **Issue:** The custom `ServerCertVerifier` returns `ServerCertVerified::assertion()` and `HandshakeSignatureValid::assertion()` unconditionally. This is a documented design decision (identity is pinned at the protocol layer: Ed25519 signature over the TLS exporter + SCRAM server signature, spec §6.3/§3.3), and the e2e tests exercise the pin path via `HandshakeBuilder::pinned_hash`. But the only thing standing between a downstream caller and a fully unauthenticated, indistinguishable-from-TLS channel is a doc comment — this is a public, one-call API with zero structural enforcement, and `verify_tls13_signature` → assertion means even proof-of-possession of the presented cert's key is skipped, so TOFU-style pinning *at the TLS layer* is impossible by construction.
- **Failure scenario:** a tool, REPL, or new client built on `make_client_config_no_ca()` that skips (or mis-orders) the `process_auth_ok` pin/signature verification silently accepts any attacker-presented self-signed cert; the attacker completes a TLS 1.3 handshake as a MITM endpoint. Because `identity_sig` would then fail later, a careful caller is saved — but nothing forces that code to exist, and a partial integration (e.g. accepting `auth_ok` without verifying `identity_sig`/`server_pub_key` against a pin) fails open.
- **Suggested fix:** (a) verify the TLS 1.3 CertificateVerify signature against the *end-entity cert's own* public key (e.g. via `rustls-webpki`'s `EndEntityCert` without chain building) — keeps the "accept any self-signed cert" semantics while restoring proof-of-possession; (b) consider an enforcing variant that takes the pinned SPKI/pubkey hash as a parameter so the config itself refuses unpinned servers, keeping the current function as the explicitly-named TOFU escape hatch; (c) add a `# SECURITY` doc section naming the caller obligations verbatim (verify `identity_sig` over the exporter against the pinned key).

### 3. `extract_tls_exporter` collapses all errors (incl. incomplete handshake) into `None`, inviting constant/absent channel binding
- **File:line:** `crates/shamir-transport-tcp/src/tls.rs:77-83`
- **Severity:** low
- **Issue:** `.ok()?` maps every `rustls::Error` — including `HandshakeNotComplete` — into `None`. An `Option<[u8; 32]>` return invites `unwrap_or` / `unwrap_or_default` misuse at the call site; substituting a placeholder exporter would silently reduce the §4.2 channel binding to a known constant, letting a recorded proof replay across connections (the exact attack the exporter binding exists to stop).
- **Failure scenario:** a future caller treats `None` as "exporter unavailable, continue without binding" (e.g. to match `BindingMode::TlsNoExport` peers) instead of aborting; SCRAM is then bound to a constant and loses its MITM-defence.
- **Suggested fix:** return `Result<[u8; 32], ExporterError>` (thiserror enum distinguishing `HandshakeNotComplete` from cipher/algorithm errors), or at minimum document "None MUST be treated as fatal for `TlsExporter` mode" on the API. All current callers `.expect()`, so this is API hardening, not a live bug.

### 4. Public library API returns `Box<dyn Error + Send + Sync>` instead of a `thiserror` enum
- **File:line:** `crates/shamir-transport-tcp/src/tls.rs:30` and `tls.rs:44`
- **Severity:** low
- **Issue:** `generate_self_signed_server_cert` and `make_server_config_from_pem` expose `Result<_, Box<dyn Error + Send + Sync>>`, and the "no PKCS8 key in PEM" condition is surfaced as an opaque string. This breaches the project error-handling rule (CLAUDE.md: `thiserror` for library error enums with `#[from]`; `Box<dyn Error>` is a last resort for boundary code). Callers cannot distinguish "malformed key material" from "malformed cert chain" programmatically — relevant at a security boundary where an operator needs to know whether to replace the key file or the cert file.
- **Suggested fix:** a small `TlsConfigError` enum (`#[from] rcgen::Error`, `#[from] rustls::Error`, `#[from] pki_types::pem::Error`, `#[error("no PKCS8 key in PEM")] MissingKey`, ...).

### 5. Write-side frame cap is hardcoded, and the too-short-buffer error uses a misleading sentinel
- **File:line:** `crates/shamir-transport-tcp/src/framing.rs:153, 192-197, 241-255`
- **Severity:** nit
- **Issue:** `write_frame` / `write_frame_into` / `write_frame_prereserved` all enforce `MAX_FRAME_SIZE_DEFAULT` (16 MiB) while the read side takes a caller-supplied cap — the server's pre-auth tightening pattern (`MAX_PRE_AUTH_FRAME` on read in `shamir-server`) cannot be mirrored on write, and a spec-driven per-role limit has no knob. Additionally, `write_frame_prereserved` reports a `buf.len() < 4` rejection as `TooLarge { actual: 0, max: 16 MiB }` — the `actual: 0` sentinel makes the operator-facing message ("frame too large: 0 > 16777216") wrong about the failure.
- **Suggested fix:** add `max_frame_size: usize` parameters (or a `FrameLimits` struct shared by read/write), and a dedicated `FrameError::MalformedPrefix` variant for the `< 4` case.

### Coverage note (test-organization conformance)
Tests follow the mandated layout (`src/tests/mod.rs` manifest-only, sibling `listener_tests.rs`; integration tests split by topic: `framing.rs`, `tls13_only.rs`, `handshake_e2e.rs`, `echo_e2e.rs`) and are genuinely strong for this theme: TLS 1.3-only refusal in **both** directions, exporter equality across ends, full SCRAM/pin e2e, and loopback-policy refusals covering `0.0.0.0`/`::`. The one gap relevant to this theme is the missing cancellation-path test for the unsafe pooled read (Finding 1).

</details>
