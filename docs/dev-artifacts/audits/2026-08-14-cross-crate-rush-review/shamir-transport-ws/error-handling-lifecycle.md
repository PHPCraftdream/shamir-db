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
