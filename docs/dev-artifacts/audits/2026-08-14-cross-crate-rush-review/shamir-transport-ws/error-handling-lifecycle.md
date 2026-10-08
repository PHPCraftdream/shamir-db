<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-transport-ws — error-handling-lifecycle independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Owned acceptance cleanup and typed Result discipline are supported. The public rejection taxonomy is not produced by the APIs. Preserved scratch on error is a valid unspecified postcondition, not an established replay defect; WS protocol-close obligations remain incomplete.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 7 | 6 | 0 | 0 | 0 | 0 | 1 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — `WsAcceptError::OriginRejected` and `WrongPath` are dead variants — all accept rejections surface as `Handshake`

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Exact tungstenite 0.24.0 ServerHandshake converts a callback rejection into Error::Http after writing the response. The acceptor question-mark conversion produces only Handshake. HTTP status remains structurally accessible; original path/rejection details require preserving callback state.

Evidence: [crates/shamir-transport-ws/src/server.rs:55](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/server.rs#L55); [crates/shamir-transport-ws/src/server.rs:99](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/server.rs#L99); [crates/shamir-transport-ws/src/server.rs:144](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/server.rs#L144); [Cargo.lock:4467](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4467).

<a id="review-2"></a>

### Claim 2 — Error paths of framing and accept have no variant-asserting tests

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Targeted policy-rejection and malformed-message oracles are absent. Positive live/TS tests and the bare oversize is_err assertion cannot distinguish removing rejection or substituting error variants.

Evidence: [crates/shamir-transport-ws/src/tests/server_tests.rs:7](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/tests/server_tests.rs#L7); [crates/shamir-transport-ws/tests/framing_round_trip.rs:69](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/tests/framing_round_trip.rs#L69).

Grouping/duplicate: [correctness-tdd.md#2](correctness-tdd.md#review-2). This is not an additional independent defect.

<a id="review-3"></a>

### Claim 3 — Send path lacks the TCP sibling's size guard; `payload.len() as u32` truncates silently

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The WS sender has neither TCP's 16 MiB guard nor checked narrowing before allocation. Peer rejection is not a local send-validation guarantee.

Evidence: [crates/shamir-transport-ws/src/framing.rs:118](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/framing.rs#L118); [crates/shamir-transport-tcp/src/framing.rs:153](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L153).

Grouping/duplicate: [api-wire-protocol.md#3](api-wire-protocol.md#review-3). This is not an additional independent defect.

<a id="review-4"></a>

### Claim 4 — Exporter extraction failure is indistinguishable from unavailability (`Option` discards the cause)

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The wrapper preserves the TCP helper's ok()-to-Option error erasure; the native production caller substitutes zeros. A reachable failure of the real completed TLS exporter is not demonstrated, and mode remains TlsExporter.

Evidence: [crates/shamir-transport-ws/src/tls_exporter.rs:20](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/tls_exporter.rs#L20); [crates/shamir-transport-tcp/src/tls.rs:81](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tls.rs#L81); [crates/shamir-server/src/server/server_launcher.rs:1391](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_launcher.rs#L1391).

Grouping/duplicate: [security-crypto.md#5](security-crypto.md#review-5). This is not an additional independent defect.

<a id="review-5"></a>

### Claim 5 — Framing error leaves the caller's scratch buffer holding the previous frame

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

Retention is factual, but no empty-on-error guarantee exists. Validation fails before modifying a fully initialized prior buffer; production checks Err before decoding. Ignoring Result is caller misuse. Documenting the postcondition is optional API clarification, not a demonstrated remediation obligation.

Evidence: [crates/shamir-transport-ws/src/framing.rs:149](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/framing.rs#L149); [crates/shamir-transport-ws/src/framing.rs:169](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/framing.rs#L169); [crates/shamir-server/src/connection/handshake.rs:719](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/handshake.rs#L719); [crates/shamir-server/src/connection/request_loop.rs:280](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L280).

<a id="review-6"></a>

### Claim 6 — Doc attributes the 4 KiB pre-auth check to `ws_recv_into`, which enforces nothing by itself

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The adapter does enforce its supplied numeric cap; it has no intrinsic 4 KiB or authentication-phase rule. The server supplies that rule after codec assembly.

Evidence: [crates/shamir-transport-ws/src/server.rs:27](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/server.rs#L27); [crates/shamir-transport-ws/src/framing.rs:163](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/framing.rs#L163); [crates/shamir-server/src/connection/handshake.rs:717](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/handshake.rs#L717).

Grouping/duplicate: [performance-hotpath.md#4](performance-hotpath.md#review-4). This is not an additional independent defect.

<a id="review-7"></a>

### Claim 7 — `Origin` header containing obs-text bytes is reported as `Missing` rather than rejected-as-present

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Exact http 1.4.0 permits opaque header octets but to_str rejects them; ok() then yields None and Missing. The upgrade still fails. The lost distinction is present-but-invalid versus absent, not a UTF-8-only issue.

Evidence: [crates/shamir-transport-ws/src/server.rs:132](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/server.rs#L132); [crates/shamir-transport-ws/src/browser.rs:99](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/browser.rs#L99); [Cargo.lock:1672](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L1672).

## Evidence and recipe corrections

- Typed callback rejections can be retained in borrowed local state without shared locks. HTTP status alone cannot reconstruct WrongPath.actual or the full original OriginRejected value after a static-body change.
- An owned stream dropped on acceptance failure is not a resource leak. Established WS connections additionally have protocol-close and finite cleanup obligations; the blanket lifecycle-clean assurance does not prove those.
- Clearing scratch changes an unspecified error postcondition and cancellation-visible state; it is not necessary to prevent current stale-request processing.
- No native-to-browser mode downgrade follows from zero fallback, and the cited Rust-client checks do not certify every WSS client.
- Published sources inspected: tungstenite 0.24.0 src/handshake/server.rs, https://docs.rs/crate/tungstenite/0.24.0/source/src/handshake/server.rs; http 1.4.0 src/header/value.rs, https://docs.rs/crate/http/1.4.0/source/src/header/value.rs.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-transport-ws — Error handling & resource lifecycle

## Summary

Crate-level error discipline is largely faithful to CLAUDE.md: every fallible API returns `Result` over a `thiserror` enum (`#[from]` where natural), src contains zero `panic!`/`unwrap`/`anyhow`/`Box<dyn Error>`, and the bind path validates policy *before* socket creation, so the error path leaks nothing. The real defects sit in the accept layer: `WsAcceptError` advertises a three-way classification (handshake / origin / wrong-path) of which only `Handshake` is ever constructible — both policy rejections surface as tungstenite `Error::Http` — and the error paths of framing and accept are almost entirely untested (the single framing error test asserts bare `is_err()`, and no test drives either accept fn to an error). The send path also dropped the outbound size guard its TCP sibling performs before the identical `as u32` cast.

## Findings

Ranked most severe first.

### 1. `WsAcceptError::OriginRejected` and `WrongPath` are dead variants — all accept rejections surface as `Handshake`

- **File:line:** `crates/shamir-transport-ws/src/server.rs:55-71` (enum), `:99` and `:144` (the only `?` construction sites)
- **Severity:** medium
- **Issue:** Both accept fns implement wrong-path rejection (server.rs:90-94, :124-128) and origin rejection (:133-139) by returning `ErrorResponse` from the handshake callback. Tungstenite converts that into `Error::Http`, and the single `?` maps it through `#[from]` into `WsAcceptError::Handshake`. A workspace-wide grep confirms nothing ever constructs `WsAcceptError::OriginRejected(...)` or `WsAcceptError::WrongPath { .. }`; the `#[from] OriginRejected` conversion is unreachable. (The `OriginRejected` *type* is real — `validate_origin` produces it — but its typed propagation into `WsAcceptError` never happens.)
- **Failure scenario:** The first consumer wiring `/shamir/v1/browser` (e.g. shamir-server) that matches `WsAcceptError::OriginRejected(_)` to count or escalate policy denials — or to map the rejection back to HTTP 403 — never hits that arm; a security-relevant origin rejection (the endpoint's primary anti-CSWSH defence) is indistinguishable from a TLS/IO handshake failure except by string-matching the `Http` response body (`"origin rejected: …"`). Any test asserting `Err(WsAcceptError::OriginRejected(_))` cannot pass by construction.
- **Suggested fix:** After the `.await`, inspect the `Error::Http` response (status 404/403 + reason) and re-map into `WrongPath` / `OriginRejected` before returning, since the request is only visible inside the callback. Alternatively remove the dead variants and document how to classify from `Handshake(Error::Http)`. Either way, the exported error taxonomy should stop promising classification it does not deliver.

### 2. Error paths of framing and accept have no variant-asserting tests

- **File:line:** `crates/shamir-transport-ws/tests/framing_round_trip.rs:57-71` (the only framing error test — `assert!(result.is_err())` at :69, variant never checked); `crates/shamir-transport-ws/src/tests/server_tests.rs` (no rejection-path test at all); no `src/framing/tests/` exists
- **Severity:** medium
- **Issue:** `WsFrameError::LengthMismatch` (both the `<4`-byte case framing.rs:148-153 and the mismatched-prefix case :157-162), `NonBinaryMessage` (:177-182), and `PeerClose` (:144, :173) have zero coverage; `rejects_oversized_frame` would stay green if the returned variant were swapped for any other error. Server-side, no test drives `accept_native_ws` / `accept_browser_ws` to an error (wrong path, missing `Origin`, disallowed `Origin`), so both the broken classification (finding 1) and the actual HTTP 404/403 rejection responses are unverified. The covered modules are solid: `browser_tests.rs` (10 tests, all reject paths) and `listener_tests.rs` (validate-before-bind error paths) match the theme well.
- **Failure scenario:** A refactor that treats `Message::Text` as skippable (`continue`), flips the length check to `declared < body.len()`, or wraps peer-close into `Io` passes the whole suite silently — regressions in the fail-closed guarantees go unnoticed.
- **Suggested fix:** Add `src/framing/tests/framing_error_tests.rs` (module layout per CLAUDE.md) feeding raw `Message::Binary` of 3 bytes, a wrong declared length, and `Message::Text` / `Message::Close`, asserting the exact variants; tighten `rejects_oversized_frame` to `matches!(…, Err(WsFrameError::TooLarge { .. }))`; add duplex-pipe accept tests asserting the concrete `WsAcceptError` and status codes (after fixing finding 1).

### 3. Send path lacks the TCP sibling's size guard; `payload.len() as u32` truncates silently

- **File:line:** `crates/shamir-transport-ws/src/framing.rs:118-122`
- **Severity:** low
- **Issue:** `ws_send_sink` casts the payload length to `u32` with no bound check. The sibling this crate deliberately mirrors (`shamir-transport-tcp/src/framing.rs:153-159` and `:192-198`) rejects `payload.len() > MAX_FRAME_SIZE_DEFAULT` with `FrameError::TooLarge` *before* the identical cast; the WS variant dropped that guard. A payload over `u32::MAX` (or merely over the spec §8 16 MiB cap) is emitted as a frame whose declared prefix contradicts its body, and the send returns `Ok(())`.
- **Failure scenario:** A caller violating the 16 MiB cap gets success from `ws_send`; the corruption is discovered only by the peer as `LengthMismatch` (or an oversized-message reject) — the sender-side error contract is strictly weaker than the TCP transport's for the same wire format.
- **Suggested fix:** Mirror the TCP guard — `if payload.len() > MAX_WS_FRAME_SIZE { return Err(WsFrameError::TooLarge { actual: payload.len(), max: MAX_WS_FRAME_SIZE }); }` before the cast — or take `max_frame_size` as a parameter symmetric with the recv side.

### 4. Exporter extraction failure is indistinguishable from unavailability (`Option` discards the cause)

- **File:line:** `crates/shamir-transport-ws/src/tls_exporter.rs:20-22`
- **Severity:** low
- **Issue:** `extract_tls_exporter_from_stream` returns `Option<[u8; 32]>`, swallowing the `rustls::Error`. On the native endpoint the exporter always exists post-handshake (the sibling's own doc, `shamir-transport-tcp/src/tls.rs:74-76`: TLS 1.3 always supports it), so `None` here signals a broken or misused connection — per CLAUDE.md that is an error, not an absent value. With `BROWSER_CHANNEL_BINDING = [0u8; 32]` as the documented placeholder, the `Option` shape invites callers to write `.unwrap_or(BROWSER_CHANNEL_BINDING)` and silently downgrade a native connection to binding_mode=0x02 strength. Mitigating: it wraps the sibling `extract_tls_exporter`, which uses the same `Option` pattern, so the shape is at least workspace-consistent.
- **Failure scenario:** A native WSS caller maps `None` to the zero placeholder: SCRAM channel binding silently weakens with no error surfaced, and the protocol's anti-downgrade matrix will not flag it because 0x02 is a legal mode.
- **Suggested fix:** Return `Result<[u8; 32], …>` carrying the rustls cause, or minimally document in caps that `None` on the native endpoint MUST abort the connection and must never fall back to the placeholder.

### 5. Framing error leaves the caller's scratch buffer holding the previous frame

- **File:line:** `crates/shamir-transport-ws/src/framing.rs:169-170` (`buf.clear()` only on the success path)
- **Severity:** nit
- **Issue:** On `LengthMismatch` / `TooLarge` / `PeerClose`, the caller-supplied buffer still contains the *previous* frame's payload. (Fine for `ws_recv`'s fresh `Vec`; wrong only for reused scratch buffers.)
- **Failure scenario:** A caller that logs `buf` on the error path — or ignores the `Result` and proceeds — treats the previous request's bytes as the current frame's.
- **Suggested fix:** Either document that `buf` is unspecified on `Err`, or clear it at function entry so the error path is side-effect-free.

### 6. Doc attributes the 4 KiB pre-auth check to `ws_recv_into`, which enforces nothing by itself

- **File:line:** `crates/shamir-transport-ws/src/server.rs:26-28`
- **Severity:** nit
- **Issue:** The NEW-1 doc says the pre-auth 4 KiB logical check is "enforced in [`crate::framing::ws_recv_into`]" — that function takes `max_frame_size` as a caller-supplied parameter and has no 4 KiB logic; the budget lives at the framing call site. An error-enforcement claim pointing at the wrong layer invites future verification in the wrong file.
- **Suggested fix:** Rephrase to "enforced by the framing caller via `ws_recv_into`'s `max_frame_size` argument" and name the actual enforcing site.

### 7. `Origin` header containing obs-text bytes is reported as `Missing` rather than rejected-as-present

- **File:line:** `crates/shamir-transport-ws/src/server.rs:129-132` (with `browser.rs:99`)
- **Severity:** nit
- **Issue:** `to_str().ok()` maps a present-but-non-ASCII `Origin` header to `None`, so the rejection reason becomes `OriginRejected::Missing` ("browser endpoint requires Origin header"). The upgrade is still rejected (fail-closed either way), but the error label misstates reality, which matters once rejections are logged or matched on.
- **Suggested fix:** Read the raw `HeaderValue`; if `to_str()` fails, return `OriginRejected::NotAllowed` (e.g. with a `<non-utf8>` placeholder) so present-but-invalid is distinguishable from absent.

## Notes (non-findings)

- Error-path resource lifecycle is otherwise clean: `bind_validated` validates policy before socket creation (nothing to close on the reject path), and both accept fns take ownership of the stream, so any `Err` drops the socket; the crate spawns no tasks, holds no locks, and owns no files, so there is nothing else to leak on an error path.
- Library surface hygiene matches CLAUDE.md §Error handling exactly: `thiserror` everywhere, `#[from]` where natural, `?` propagation, no `anyhow`, no `Box<dyn Error>`, no panics in src (the `unwrap`/`expect`/`panic!` hits are all under `tests/`).
- Test layout follows the documented convention (crate-level `src/tests/` manifest + per-topic files); framing's tests live in the crate-root integration dir instead of a `src/framing/tests/` dir — defensible for wire-level round trips, noted only as an aside to finding 2.

</details>
