# shamir-transport-tcp -- API & wire-protocol design

## Summary

The crate's public surface is clean and consistent with the spec's shape: TLS 1.3-only configs, exporter-based channel binding, a profile-aware bind validator, and well-documented framing primitives with pooled-buffer variants; test coverage is solid (framing round-trips/size limits/Miri-annotated unsafe paths, TLS 1.3-only negotiation, loopback policy, two full TLS+SCRAM e2e tests). The builder-only query-construction rule is honored: there is no raw `serde_json` anywhere in the crate, protocol objects flow through `shamir-connect` builders (`HandshakeBuilder`, envelope types), and the test-local msgpack `Wire*` structs fall under the documented wire-format-test exception. The main gaps are an incoherent empty-frame contract (module doc vs. spec §2 vs. reader behavior), `Box<dyn Error>` library error APIs in `tls.rs` contrary to the project's documented error-handling rules, and several API-asymmetry/diagnosability issues in `FrameError` and the write-side cap.

## Findings

### 1. Write API can emit the reserved close marker as a data frame; module doc contradicts spec §2
- File: `crates/shamir-transport-tcp/src/framing.rs:8-9` (doc), `:149-166` (`write_frame`), `:187-206` (`write_frame_into`), `:247-250` (`write_frame_prereserved`)
- Severity: medium
- Issue: The module doc says "`length == 0` is a graceful close indicator. Empty frames are also legal at the application level (caller decides)." These two sentences cannot both hold: `read_frame`/`read_frame_into` map `len == 0` exclusively to `Err(FrameError::PeerClose)` (`framing.rs:53-55`, `:107-109`), so an "empty frame" can never be delivered as data — and `docs/guide-docs/client-server-protocol-spec/TRANSPORT_TCP.md` (lines 28, 106) defines `length=0` as graceful close only. Meanwhile all three write functions accept a zero-length payload (and `write_frame_prereserved` accepts a 4-byte all-zero buffer), emitting the wire close marker.
- Failure scenario: any caller that writes an empty payload through the generic `&[u8]` API (the framing layer is transport-generic; only rmp-serde's non-empty output saves the current callers) silently signals graceful close — the peer's request loop tears down the connection and subsequent requests hang until timeout, with no error on the writer side.
- Suggested fix: delete the "Empty frames are also legal" sentence, and reject zero-length payloads in `write_frame`/`write_frame_into`/`write_frame_prereserved` (e.g. a dedicated `FrameError::EmptyFrame`), reserving `length=0` for `write_close` only. This makes the writer fail closed instead of sending a control frame as data.

### 2. Library error APIs use `Box<dyn Error + Send + Sync>` instead of a `thiserror` enum
- File: `crates/shamir-transport-tcp/src/tls.rs:30` (`generate_self_signed_server_cert`), `:44` (`make_server_config_from_pem`)
- Severity: medium
- Issue: CLAUDE.md's error-handling rules are explicit: "`thiserror` for library error enums (with `#[from]` where natural)" and "`Box<dyn Error>` is a last resort for boundary code." This is a library crate (consumed by `shamir-server`, `shamir-client`, `shamir-transport-ws`), the crate already depends on `thiserror` and uses it correctly for `FrameError`/`ListenerBindError` — but the two TLS setup functions return `Result<_, Box<dyn std::error::Error + Send + Sync>>`.
- Failure scenario: callers cannot `match` on failure kinds (bad PEM vs. missing key vs. rcgen/`with_single_cert` failure) to produce targeted operator-facing config errors; they must string-match messages, which breaks across rustls/rcgen upgrades.
- Suggested fix: add a `TlsConfigError` thiserror enum with `#[from]` variants (PEM parse, missing key, rcgen, rustls config) and return it from both functions, matching the crate's own `FrameError`/`ListenerBindError` pattern.

### 3. `FrameError::TooLarge` is overloaded for non-size violations in `write_frame_prereserved`
- File: `crates/shamir-transport-tcp/src/framing.rs:241-246` (buffer < 4 bytes), `:250-255` (declared/actual mismatch)
- Severity: low
- Issue: `FrameError::TooLarge` is documented as "Frame larger than the negotiated maximum", but `write_frame_prereserved` reuses it for (a) a too-short buffer — reported as `TooLarge { actual: 0, max: 16777216 }`, i.e. a message that literally asserts `0 > 16777216` — and (b) a length-prefix/payload mismatch, which is a malformed-buffer condition unrelated to size.
- Failure scenario: a caller bug (prefix not patched, buffer truncated) surfaces in logs as "frame too large: 999 > 16777216", sending an operator hunting for a size-limit problem that does not exist.
- Suggested fix: add a `FrameError::MalformedPrefix { declared, actual }` (or `PrefixTooShort`) variant for the defensive checks; keep `TooLarge` for genuine size-cap violations.

### 4. Write-side frame cap is hardcoded to `MAX_FRAME_SIZE_DEFAULT`; reader/writer API asymmetry
- File: `crates/shamir-transport-tcp/src/framing.rs:153`, `:192`, `:250`
- Severity: low
- Issue: `read_frame`/`read_frame_into` take a `max_frame_size: usize` parameter, but all three write variants hardcode `MAX_FRAME_SIZE_DEFAULT`. The writer cannot enforce a cap smaller (or, if the spec ever allows it, larger) than the compiled-in default.
- Failure scenario: a future per-connection/negotiated smaller cap would let a caller emit frames the peer unconditionally kills the connection for (spec §2/§8: "Frame too large → TCP close без reply") — the API offers no way to check the negotiated limit at write time.
- Suggested fix: add `max_frame_size` parameters (or `write_frame_capped` companions) to the write variants; existing call sites pass `MAX_FRAME_SIZE_DEFAULT`.

### 5. Normative loopback predicate is not reusable, so shamir-server duplicates spec §2.2 policy
- File: `crates/shamir-transport-tcp/src/listener.rs:51-56`, `:89-94`; consumer: `crates/shamir-server/src/config.rs:841-850`
- Severity: low
- Issue: `validate_addr`/`ListenerProfile::allows` require the crate's `ListenerProfile` enum, which the server's own `ProfileKind` does not map to; shamir-server therefore re-implements the loopback predicate inline (its comment says exactly this). The TRANSPORT_TCP §2.2 NORMATIVE policy now lives in two independently-maintained copies, and the crate's `is_loopback` helper is private.
- Failure scenario: a future spec change to the allowed loopback range (or additional address classes) updated in one copy but not the other yields two validators that disagree about the same bind address.
- Suggested fix: expose the pure predicate (`pub fn is_loopback_ip(ip: IpAddr) -> bool` in `listener.rs`, spec-cited) so `validate_addr` and shamir-server share one implementation; optionally add `impl From<ListenerProfile>`-style interop with the server's profile type.

### 6. `extract_tls_exporter` collapses failure causes into `Option`
- File: `crates/shamir-transport-tcp/src/tls.rs:77-83`
- Severity: low
- Issue: The function returns `Option<[u8; 32]>`, discarding `rustls::Error` from `export_keying_material` (handshake-not-finished vs. export-refused are indistinguishable), and hardcodes the 32-byte output size without saying callers must keep it in sync with the spec's channel-binding length.
- Failure scenario: debugging a channel-binding mismatch ("exporter is None on one side") offers no diagnostic beyond a swallowed error; both e2e tests already hit the unwrap-or-panic path (`expect("exporter")`).
- Suggested fix: return `Result<[u8; 32], rustls::Error>` (or a small thiserror wrapper), and document the fixed 32-byte contract relative to spec §4.2.

### 7. Uneven crate-root re-exports: the recommended pooled API is not at the root
- File: `crates/shamir-transport-tcp/src/lib.rs:12`, `:16-19`
- Severity: low
- Issue: `lib.rs` re-exports `read_frame`/`write_frame` but not `read_frame_into`/`write_frame_into`/`write_frame_prereserved`/`write_close` — even though the framing docs steer hot-path callers to the `*_into`/prereserved variants — and re-exports only 4 of 7 public `tls` items (`ConnectionExporter`, `EXPORTER_LABEL`, `EXPORTER_CONTEXT` are root-absent). Consumers end up with mixed paths (`shamir_transport_tcp::framing::write_frame` in `shamir-client/src/tests/demux_tests.rs:16` vs. the root re-export elsewhere).
- Failure scenario: discoverability/consistency only; no runtime impact.
- Suggested fix: either re-export the full public surface of each module at the crate root, or drop the root re-exports and commit to the module paths as the single canonical route.

### 8. Stale doc: `extract_tls_exporter` claims to be generic over `rustls::ConnectionTrait`
- File: `crates/shamir-transport-tcp/src/tls.rs:73-74`
- Severity: nit
- Issue: The doc says "Generic over `T: rustls::ConnectionTrait`", but the signature is generic over the crate-local `ConnectionExporter` trait (`tls.rs:88-96`); the accompanying rationale ("avoids importing rustls's stable trait") is undercut by the trait's signature referencing `rustls::Error` anyway.
- Failure scenario: none at runtime; doc misleads contributors about the extension point (e.g. `shamir-transport-ws/src/tls_exporter.rs` must impl the local trait, not rustls's).
- Suggested fix: reword to "generic over `ConnectionExporter` (impl'd for both `tokio_rustls` stream halves)" and note that WS/other backends add their own impl.

### 9. Docs advertise Unix-domain-socket binds for `Plain`; the API is `SocketAddr`/TCP-only
- File: `crates/shamir-transport-tcp/src/listener.rs:4-6`, `:30-35`
- Severity: nit
- Issue: Both the module doc and `ListenerProfile::Plain`'s doc say Plain is permitted on loopback "or Unix domain sockets", but the crate offers only `SocketAddr`-based validation/bind; no UDS path or `allows_unix` predicate exists.
- Failure scenario: an operator or contributor reads the spec-guided docs, expects a UDS option, and finds no API to express or validate one.
- Suggested fix: either drop the UDS clause from these docs until a UDS bind helper lands, or add an explicit `ListenerProfile::allows_unix_path()` / `bind_validated_unix` stub with the same fail-closed policy.

### 10. Speculative public constants carry vestigial `#[allow(dead_code)]`
- File: `crates/shamir-transport-tcp/src/listener.rs:96-100`
- Severity: nit
- Issue: `LOOPBACK_V4`/`LOOPBACK_V6` are `pub` yet annotated `#[allow(dead_code)]` (dead_code cannot fire on public lib items — the allows are leftovers from when they were private) and are referenced nowhere in the workspace.
- Failure scenario: none; minor API-surface noise implying an in-crate consumer that does not exist.
- Suggested fix: remove the attributes; if nothing adopts them, consider deleting the constants (or fold them into the reusable predicate from finding 5's fix).

### 11. `tokio` dependency pulls `features = ["full"]` in a library crate
- File: `crates/shamir-transport-tcp/Cargo.toml:14`
- Severity: nit
- Issue: A transport library only needs `net`, `io-util`, `rt` (and `macros`/`time` for dev/tests), but `features = ["full"]` unions signal/process/fs/etc. into every downstream build via feature unification.
- Failure scenario: compile-time/binary-surface bloat for consumers; masks which tokio capabilities the crate actually uses.
- Suggested fix: declare the minimal feature set in `[dependencies]` and the extras under `[dev-dependencies]`.

### 12. Transport wire structs and fixtures duplicated across the two e2e test files
- File: `crates/shamir-transport-tcp/tests/handshake_e2e.rs:41-111`, `tests/echo_e2e.rs:57-146`
- Severity: nit
- Issue: `WireAuthInit`/`WireChallenge`/`WireClientProof`/`WireAuthOk`, `fast_kdf()`, and `make_user()` are copy-pasted between the two integration tests; each file independently defines what it asserts to be the transport-local wire format.
- Failure scenario: a spec §6 envelope change updated in one file but not the other leaves both tests green while they encode two different wire formats.
- Suggested fix: share the `Wire*` structs and fixtures via a `tests/common/mod.rs`-style helper (or move them into a `#[cfg(test)]` unit module under `src/tests/`), so the wire shapes are defined once.
