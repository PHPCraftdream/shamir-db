# shamir-transport-tcp -- Correctness & TDD-coverage

## Summary

The crate is small, logically tight, and well-tested for its happy paths: framing has boundary tests (at-limit, oversized, partial reads) plus dedicated Miri tests for the one `unsafe` block, and the TLS 1.3-only normative requirement is tested in both directions. The gaps are on the edges the tests don't reach: the length==0 close sentinel can be emitted accidentally by a zero-length `write_frame` (while the module doc claims empty frames are expressible), `tls.rs` has no unit tests and its error branches are never exercised, the write-side size cap is hardcoded where the read side takes a negotiated parameter, and one listener test's name promises coverage its body deliberately does not deliver. No critical or high correctness bugs found; the `unsafe set_len` path in `read_frame_into` is correctly guarded and Miri-vetted.

## Findings

### 1. Zero-length payload write silently emits the close sentinel; module doc claims empty frames are expressible

- **File:line:** `crates/shamir-transport-tcp/src/framing.rs:8-9` (doc), `:53-55`, `:107-109` (read-side PeerClose mapping), `:153-166` (unguarded `write_frame`)
- **Severity:** medium
- **Issue:** The module doc states "`length == 0` is a graceful close indicator. Empty frames are also legal at the application level (caller decides)." But the wire-level `length == 0` is exclusively the close sentinel: `read_frame`/`read_frame_into` *always* map it to `Err(PeerClose)`, so an "empty frame" can never be delivered to a caller as `Ok(vec![])`. Meanwhile `write_frame(w, &[])` succeeds (0 <= cap) and writes `00 00 00 00` — a close request — with no guard and no test pinning this behavior.
- **Failure scenario:** Any handler that serializes to zero bytes and calls `write_frame` (or a future caller following the doc's "empty frames are legal" claim) makes the *peer's* read return `PeerClose`. Downstream consumers treat that as graceful close and tear down the loop (`shamir-client/src/client.rs:227-230` breaks on `PeerClose`; `tests/echo_e2e.rs:305` breaks on any frame error) — a silent connection drop that looks like the *other* side asked to close.
- **Suggested fix:** Either (a) reject empty payloads in `write_frame`/`write_frame_into` with a dedicated `FrameError::EmptyPayload` so the close sentinel can only be produced by `write_close`, or (b) correct the module doc to state that length==0 is *only* the close sentinel and empty frames are not expressible. Add a test whichever way is chosen — today the documented contract is false and untested.

### 2. `tls.rs` has zero unit tests; all error branches unexercised (happy-path-only e2e coverage)

- **File:line:** `crates/shamir-transport-tcp/src/tls.rs:41-58` (`make_server_config_from_pem` error paths), `:77-83` (`extract_tls_exporter` `None` path); no `tests/` dir exists for the module
- **Severity:** medium
- **Issue:** The `tls` module is covered only indirectly by e2e tests (`tests/handshake_e2e.rs`, `tests/echo_e2e.rs`, `tests/tls13_only.rs`), all of which drive the success path. Never executed by any test: PEM parse failure (`CertificateDer::pem_slice_iter` error), missing-PKCS8-key path (`ok_or("no PKCS8 key in PEM")` at `tls.rs:48-50`), key/cert mismatch (`with_single_cert` error), and `extract_tls_exporter` returning `None` for a handshake that isn't complete. Against CLAUDE.md's Red/Green/Refactor protocol these library error branches have no failing-test-first record at all — they are the most likely real-world misconfiguration surface (persisted cert/key file corrupted or rotated out of sync, per `generate_self_signed_server_cert`'s "caller persists" contract).
- **Failure scenario:** A corrupted or swapped cert/key PEM at server boot yields some `Err` — but nothing pins *which* error, or that a wrong-format key (e.g. PKCS#1 `BEGIN RSA PRIVATE KEY`) is rejected rather than silently mis-parsed.
- **Suggested fix:** Add a `tests/` unit-test file for `tls.rs` (per CLAUDE.md test layout) covering: valid round-trip already exists via e2e; add bad-PEM, empty-PEM, no-PKCS8-block, cert/key-mismatch → `Err`, and a pre-handshake exporter extraction → `None` (can be done with a `rustls` ConnectionCommon that hasn't completed a handshake).

### 3. `write_frame_prereserved` reports buffers shorter than 4 bytes as `TooLarge { actual: 0 }` — wrong error semantics, enshrined by test, worked around downstream

- **File:line:** `crates/shamir-transport-tcp/src/framing.rs:241-255`; test enshrining it: `tests/framing.rs:309-324` (esp. 315-316); downstream workaround: `crates/shamir-server/src/framer.rs:118-122, 190-193`
- **Severity:** low
- **Issue:** A `buf.len() < 4` input is a *malformed* argument, not an oversized frame, yet it returns `FrameError::TooLarge { actual: 0, max: 16 MiB }`. Same for the declared-vs-actual mismatch (`declared_len != actual_payload` at `:250`). `FrameError` has no `Malformed` variant, so two distinct programmer bugs collapse into a misleading "frame too large" classification. The test `write_frame_prereserved_rejects_malformed_buffers` asserts the wrong variant, cementing it; and `shamir-server`'s `FrameWriter::write_frame_prereserved` default impls already pre-check `buf.len() < 4` and return their own `FramerError::Decode` — evidence the transport-level error was unusable as-is (the TCP override at `framer.rs:244-245` still surfaces the mislabeled variant).
- **Failure scenario:** A caller bug building the prereserved buffer produces `frame too large: 0 > 16777216` in logs/metrics — actively misleading during incident triage.
- **Suggested fix:** Add `FrameError::Malformed { reason }` (or a dedicated `ShortPrefix` variant) for both guard branches; update the test to assert the new variant; drop the now-redundant pre-checks in `shamir-server`'s framer or leave them as defense-in-depth.

### 4. Write-side frame cap is hardcoded to `MAX_FRAME_SIZE_DEFAULT`; read side takes a caller-supplied max

- **File:line:** `crates/shamir-transport-tcp/src/framing.rs:153-158` (`write_frame`), `:192-197` (`write_frame_into`), `:250` (`write_frame_prereserved`)
- **Severity:** low
- **Issue:** `read_frame`/`read_frame_into` accept a `max_frame_size` parameter (doc calls it "the negotiated maximum"), but every write-side function checks against the compile-time `MAX_FRAME_SIZE_DEFAULT` only. If the spec's `MAX_FRAME_SIZE_DATA` is ever negotiated down per-connection (the read-side API is clearly built for that), a writer has no way to honor it and will happily emit frames the peer rejects as `TooLarge`, killing the connection asymmetrically.
- **Failure scenario:** Future negotiated cap of e.g. 1 MiB: local `write_frame` succeeds with a 4 MiB payload; peer's `read_frame(1 MiB)` returns `TooLarge` and drops the connection — a bug that only manifests cross-version.
- **Suggested fix:** Thread `max_frame_size` through the write functions (or take it once in a `Framer`-style struct). If the cap is genuinely fixed by spec forever, add a comment on the read-side parameter saying so, so the asymmetry is documented rather than latent.

### 5. Test name promises TLS-bind-on-unspecified coverage the body deliberately doesn't deliver

- **File:line:** `crates/shamir-transport-tcp/src/tests/listener_tests.rs:100-106`
- **Severity:** low
- **Issue:** `bind_validated_succeeds_for_tls_on_unspecified` binds `127.0.0.1:0`, not the unspecified address — the inline comment admits it ("use loopback to avoid firewall prompts in CI"). The test is vacuous relative to its name: "a TLS profile may really bind `0.0.0.0`" is never verified end-to-end; only the pure predicate `allows()` is unit-tested (`listener_tests.rs:58-65`). A future regression in `bind_validated`'s pass-through (e.g. accidentally applying the loopback gate to all profiles) would still pass this test's TLS branch as written.
- **Suggested fix:** Rename to `bind_validated_succeeds_for_tls_on_loopback` and add a separate test that calls `bind_validated` with `0.0.0.0` + `TlsExporter` (or, if binding `0.0.0.0` is genuinely off-limits in CI, assert only `validate_addr(0.0.0.0, Tls).is_ok()` under a name that says so).

### 6. TLS config constructors panic if no process-level rustls `CryptoProvider` is installed; `make_client_config_no_ca`'s infallible signature hides it

- **File:line:** `crates/shamir-transport-tcp/src/tls.rs:54` (`ServerConfig::builder_with_protocol_versions`), `:63-69` (`make_client_config_no_ca` returning `Arc<ClientConfig>` with no `Result`)
- **Severity:** low
- **Issue:** rustls 0.23's `builder_with_protocol_versions` uses the process-default crypto provider and panics when none is installed. These are `pub` library APIs, so the panic depends on caller-side global state (`install_default()`). CLAUDE.md's error rules say return `Result<T, E>` and reserve panics for programmer bugs; here a mere forgotten one-line setup step in a consumer's binary panics deep inside this crate. Every in-repo caller must remember it — there are 60+ scattered `install_default()` call sites workspace-wide, and `shamir-client` even installs the provider inside its own library code (`shamir-client/src/client.rs:377-379`), which demonstrates the trap is real.
- **Failure scenario:** An external consumer links this crate, calls `make_client_config_no_ca()` before installing a provider → panic at runtime instead of a typed error.
- **Suggested fix:** Minimum: document the requirement on both constructors ("panics unless a process-level CryptoProvider is installed"). Better: accept `&Arc<CryptoProvider>` parameters (rustls supports `builder_with_provider`) or return `Result`, keeping the current fns as thin panicking wrappers if call-site churn is a concern.

### 7. Write-path boundary branches and EOF-vs-close distinction untested

- **File:line:** `crates/shamir-transport-tcp/src/framing.rs:153-158`, `:192-197`, `:250` (untested `TooLarge` branches); `:49-50` (EOF path); `crates/shamir-transport-tcp/src/listener.rs:51-56` (IPv4-mapped IPv6 edge)
- **Severity:** low
- **Issue:** The read side's size boundary is well tested (`rejects_oversized_frame_declaration`, `frame_exactly_at_size_limit_is_accepted`, `read_frame_into_rejects_oversized`), but no test ever executes the write-side rejections (payload > 16 MiB in `write_frame`/`write_frame_into`; `declared_len > MAX_FRAME_SIZE_DEFAULT` in `write_frame_prereserved` — the second half of the `||` at `:250` is dead in tests). Also untested: abrupt EOF (peer vanishes without the close frame) yielding `FrameError::Io(UnexpectedEof)` — the distinction consumers rely on (`shamir-client/src/client.rs:227-234` treats `PeerClose` and other errors differently) — and the IPv4-mapped IPv6 input `::ffff:127.0.0.1`, which std's `is_loopback()` classifies as non-loopback (fail-closed, so correct, but the edge is undocumented and unpinned).
- **Failure scenario:** A refactor of the write-side cap check (e.g. dropping the `declared_len > MAX` clause as "unreachable") would be green — nothing would catch it.
- **Suggested fix:** Add cheap tests: `write_frame` with 16 MiB + 1 (one allocation, tolerable in a test), a prereserved buffer with a valid prefix but > MAX payload, a `duplex` dropped mid-frame → `Io` error, and `Plain.allows(::ffff:127.0.0.1) == false` to pin the fail-closed behavior.

### 8. `Box<dyn Error + Send + Sync>` as the public error surface of `tls.rs` constructors

- **File:line:** `crates/shamir-transport-tcp/src/tls.rs:30`, `:44`
- **Severity:** nit
- **Issue:** CLAUDE.md prescribes `thiserror` for library error enums and calls `Box<dyn Error>` a last resort for boundary code. `generate_self_signed_server_cert` / `make_server_config_from_pem` are library APIs whose failure modes are enumerable (rcgen, PEM parse, missing key, rustls cert/key mismatch) and map naturally onto a `thiserror` enum with `#[from]`. (May overlap the error-handling reviewer's lens; listed here because it also degrades testability of finding #2 — callers can't `matches!` on specific causes.)
- **Suggested fix:** Introduce a `TlsConfigError` thiserror enum with `#[from]` variants for `rcgen::Error`, pki-types PEM errors, and `rustls::Error`; keep the string-y "no PKCS8 key in PEM" case as a typed variant.

### 9. Redundant `#[allow(dead_code)]` on `pub` consts; unit tests centralized in `src/tests/` instead of per-module

- **File:line:** `crates/shamir-transport-tcp/src/listener.rs:96-100` (`LOOPBACK_V4`/`LOOPBACK_V6`); `crates/shamir-transport-tcp/src/tests/` (layout)
- **Severity:** nit
- **Issue:** The `#[allow(dead_code)]` attributes on `pub const` items in a `pub mod` are no-ops (public items are never dead-code-flagged) — likely leftovers from an iteration where they were private, suggesting they never found their consumer. Separately, CLAUDE.md's test-organisation section specifies "one `tests/` directory per module" (`src/<module>/tests/`); this crate centralizes all unit tests in a crate-level `src/tests/` (manifest-only `mod.rs` and correct wiring in `lib.rs:13-14` are otherwise fully compliant, and no inline `#[cfg(test)]` blocks exist — good).
- **Suggested fix:** Delete the redundant allows or give the consts a real consumer; when the module set grows, split `src/tests/` into per-module `tests/` dirs to match the documented layout.
