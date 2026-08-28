# shamir-transport-tcp -- Style & CLAUDE.md structural conformance

## Summary

Structural conformance is largely strong: `src/tests/mod.rs` is a manifest-only `mod.rs`, no inline `#[cfg(test)] mod tests` exists in any implementation file, `lib.rs` holds only module declarations, test wiring, and re-exports, and all four `src/` files keep imports at the top. The two real drifts are at the crate edges: the `lib.rs` re-export block is partial, stale relative to the framing/tls APIs added later, and used by zero consumers (everyone imports via module paths), and five function-local `use` statements in the `tests/` files breach the mandatory imports-at-top rule with no documented exception applying. Remaining items are minor: a wire-format constant duplicated from a direct dependency, two speculative dead public consts, and a few inaccurate doc comments.

## Findings

### 1. Crate-root re-exports are partial, stale, and used by nobody
- **File:line:** `src/lib.rs:12-19` (vs `src/framing.rs`, `src/tls.rs`)
- **Severity:** medium
- **Issue:** `pub use framing::{read_frame, write_frame, FrameError, MAX_FRAME_SIZE_DEFAULT}` and `pub use tls::{extract_tls_exporter, generate_self_signed_server_cert, make_client_config_no_ca, make_server_config_from_pem}` snapshot only the initial-cut API. Everything added since is missing from the root: `read_frame_into` (framing.rs:98), `write_frame_into` (:187), `write_frame_prereserved` (:233), `write_close` (:209), the `ConnectionExporter` trait (tls.rs:88), and `EXPORTER_LABEL`/`EXPORTER_CONTEXT` (tls.rs:18,20). A workspace-wide grep finds **zero** imports through the root paths -- every consumer (`shamir-server`, `shamir-client`, `shamir-transport-ws`, and this crate's own tests/bench) uses `shamir_transport_tcp::framing::*` / `::tls::*` / `::listener::*`.
- **Failure scenario:** a reader treats the root block as the canonical API surface, writes `use shamir_transport_tcp::read_frame_into;`, and fails to compile; conversely, each new public framing/tls item silently widens the gap, leaving two divergent import paths for the same crate.
- **Suggested fix:** delete the root re-export block (module paths are the de-facto canonical surface) or complete it. If kept, also move `#[cfg(test)] mod tests;` (lib.rs:13-14) up beside the `pub mod` declarations so the file reads module decls -> test wiring -> re-exports instead of interleaving.

### 2. Function-local `use` statements violate the imports-at-top rule (5 sites)
- **File:line:** `tests/framing.rs:50` and `tests/framing.rs:155`; `tests/tls13_only.rs:36-38`; `tests/echo_e2e.rs:208`
- **Severity:** medium
- **Issue:** CLAUDE.md ("Imports at the top") mandates all `use` statements live in the file header, with three documented exceptions -- none applies here:
  - `tests/framing.rs:50,155` -- `use tokio::io::AsyncWriteExt;` inside `rejects_oversized_frame_declaration` and `read_frame_into_rejects_oversized`. No collision exists; hoisting is trivial (other tests in the same file call the same trait methods through helper APIs without it).
  - `tests/tls13_only.rs:36-38` -- three imports inside `tls12_only_server_config`, while the file header (line 11) already imports `CertificateDer` from the very same `rustls::pki_types` path, making the merge mechanical.
  - `tests/echo_e2e.rs:208` -- `use shamir_connect::common::latency::{target_constant_time_ms, LatencyPadGuard};` inside the server-task async block; the surrounding comment explains the latency padding, not any import-scope reason.
  All `src/` files are clean -- the violations are confined to `tests/`.
- **Failure scenario:** none functional; a direct, repeated breach of a documented mandatory convention, inconsistent with the crate's otherwise clean `src/` imports.
- **Suggested fix:** hoist all five imports to their file headers.

### 3. Spec wire constant duplicated from a direct dependency
- **File:line:** `src/tls.rs:18` (dup of `crates/shamir-connect/src/common/domain_tags.rs:35`)
- **Severity:** low
- **Issue:** `EXPORTER_LABEL: &[u8] = b"EXPORTER-ShamirDB-AUTH-v1"` re-defines `shamir_connect::common::domain_tags::TLS_EXPORTER_LABEL` byte-for-byte, and this crate already depends on `shamir-connect` (`Cargo.toml:11`). Relatedly, `EXPORTER_CONTEXT` (tls.rs:20) is `pub` with no external consumer anywhere in the workspace.
- **Failure scenario:** the label is versioned ("...-AUTH-v1"); if a v2 label lands in `domain_tags` and this copy is missed, client and server derive channel-binding material under different labels -- a silent protocol mismatch that an e2e test mocking both ends through these same constants would not catch.
- **Suggested fix:** `use shamir_connect::common::domain_tags::TLS_EXPORTER_LABEL;` (re-export under the `tls::` path if stability is needed) and make `EXPORTER_CONTEXT` private.

### 4. Speculative dead public consts with cargo-cult `#[allow(dead_code)]`
- **File:line:** `src/listener.rs:96-100`
- **Severity:** low
- **Issue:** `LOOPBACK_V4` / `LOOPBACK_V6` are referenced nowhere in the workspace -- not even by this crate's own tests (`src/tests/listener_tests.rs` builds addresses by string parsing), despite the comment "Common loopback addresses for documentation / examples / tests". The `#[allow(dead_code)]` attributes are pure noise: dead_code never fires on `pub` items reachable from a lib crate root, so they can never be needed. Both constants also restate std (`IpAddr::V4(Ipv4Addr::LOCALHOST)`, `IpAddr::V6(Ipv6Addr::LOCALHOST)`).
- **Failure scenario:** none functional; misleading public surface plus allow-attributes that advertise expected dead code.
- **Suggested fix:** delete both consts, or keep them non-`pub` and actually use them in `listener_tests.rs`.

### 5. In-src `tests/` layout covers only one of three modules
- **File:line:** `src/tests/mod.rs:1`
- **Severity:** low
- **Issue:** `src/tests/` exists and conforms to the manifest-only rule, but declares only `listener_tests`. `framing.rs` has extensive pure-logic codec tests -- including the explicitly miri-safe `std::io::Cursor` pair (`tests/framing.rs:203-250`) that needs no I/O at all -- living at the crate-root `tests/`, and `tls.rs` has no unit-level tests under `src/tests/` at all. The "one `tests/` directory per module" layout is only partially realized; crate-root `tests/` is legitimate for socket-bound e2e coverage, but the pure-logic cases have two competing homes.
- **Failure scenario:** none; consistency drift -- a contributor adding a framing unit test finds no in-src precedent and follows the integration-file pattern instead.
- **Suggested fix:** move the pure codec tests (at minimum the Cursor/miri pair) into `src/tests/framing_tests.rs` and add a `tls_tests.rs` for the pure config builders; keep the socket-bound e2e files in `tests/`.

### 6. `ConnectionExporter` doc comment misdescribes who implements what
- **File:line:** `src/tls.rs:85-88`
- **Severity:** nit
- **Issue:** "Trait that tokio-rustls + rustls connections both implement to expose `export_keying_material`" is inaccurate -- the streams do not implement this trait; it is a local adapter whose impls for `tokio_rustls::server::TlsStream` / `client::TlsStream` appear just below (lines 98, 111). The claimed benefit ("avoids importing rustls's stable trait, which would tightly couple us") is undercut by the impl bodies calling rustls's own `export_keying_material` directly and by the concrete impls being on tokio-rustls types anyway.
- **Suggested fix:** reword to describe the adapter pattern ("Local adapter trait; impls below bridge tokio-rustls client/server streams to the rustls exporter call").

### 7. `ListenerProfile::Plain` doc promises Unix-socket support the type cannot express
- **File:line:** `src/listener.rs:30-35`
- **Severity:** nit
- **Issue:** the variant doc says Plain is permitted on "loopback addresses or Unix domain sockets", but the enforcement surface (`ListenerProfile::allows(&SocketAddr)`, `bind_validated(SocketAddr, ...)`) can only ever see TCP addresses; no UDS path exists in this crate. The comment over-promises relative to the code.
- **Suggested fix:** drop the UDS clause or qualify it ("UDS support is out of scope for this binding").

### 8. Mixed spellings of the same upstream crate in one import block
- **File:line:** `src/tls.rs:10,12-13`
- **Severity:** nit
- **Issue:** the header imports the same upstream module via two paths -- `rustls::pki_types::{...}` (rustls's re-export, line 10) and `rustls_pki_types::{...}` (the direct dependency, lines 12-13) -- in the same file.
- **Suggested fix:** pick one spelling (prefer the direct `rustls_pki_types` paths, matching the RUSTSEC-2025-0134 rationale in `Cargo.toml:21-23`) and note the choice.

### Conformance confirmed (so siblings need not re-flag)
- `src/tests/mod.rs` is a manifest-only `mod.rs`; `#[cfg(test)] mod tests;` wired from the parent (`lib.rs:13-14`); no inline test modules in any implementation file.
- One-file-one-export holds: `framing.rs` is a closely-coupled codec group (one error enum + const + 6 fns over one wire format); `tls.rs` is a coherent TLS-wiring group; `listener.rs` is one profile/policy concern.
- Imports-at-top clean in all four `src/` files; bench uses `bench_scale_tool::Harness` + `bench_batched_async` (no Criterion); `thiserror` for both public error enums; doctests disabled crate-wide (`Cargo.toml` `[lib] doctest = false`) with doc examples marked `rust,ignore`.
