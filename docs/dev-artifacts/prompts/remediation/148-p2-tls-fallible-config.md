# P0 stage D6 — p2-tls-fallible-config

## Goal

Card `RV-TRANSPORT-TCP-0A23323D` (TLS convenience builders retain conditional provider/suite panic paths) in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P0.md` (read it whole). Binding ruling (two rush reviewers + @oh): NOT a stock-workspace startup failure (rustls 0.23.46 with a single aws-lc-rs provider auto-installs), but hardening: replace implicit provider selection with explicit, fallible TLS configuration.

Acceptance:
- In `crates/shamir-transport-tcp/src/tls.rs` use an explicit AWS-LC provider: `ServerConfig/ClientConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider())).with_protocol_versions(&[&rustls::version::TLS13])?` on BOTH sides; no `unwrap`/`expect` on provider or version validation; TLS 1.3 only (unchanged behaviour).
- The client config builder (`make_client_config_no_ca` and any sibling) returns `Result` and the error is propagated through every caller in shamir-client and shamir-server (change their signatures/propagation minimally; no behaviour change on success).
- Replace `Box<dyn Error + Send + Sync>` in these library APIs by a `thiserror` enum if the module already has an error type to extend (card RV-TRANSPORT-TCP-7DFC88FF) — only if it is a small local change; otherwise leave and say so.
- Tests (names contain `rv_transport_tcp_0a23323d`): config builds successfully WITHOUT any prior global provider installation; malformed server credentials return an error instead of panicking; building a config is independent of an incompatible preinstalled global provider (install a different provider first via a `rustls::crypto::CryptoProvider::install_default`-style helper in the test ONLY if that cannot poison other tests in the same process — otherwise state why the test is not possible and test the explicit-provider path directly).
- Record in the final message the exact locked rustls version evidence from Cargo.lock.

## Requirements

1. Before editing, confirm each mechanism in current code (read callers and guards). If a card is already fixed or wrong at HEAD, do not change code for it: say so with file:line evidence in the final message.
2. Fix at the narrowest layer that owns the invariant. No unrelated refactors, renames or comment edits.
3. TDD: for every card add a regression test whose name contains the card's hex suffix in lower case (for example `rv_storage_98f6650d_...`). The test must fail on the unfixed code and pass on the fix — reason about that from the code, you cannot run it. Tests live in the module's `tests/` folder (`tests/mod.rs` is a manifest only; no inline `#[cfg(test)] mod tests {}`). Prefer deterministic constructions (injected failures, barriers, paused clocks) over sleeps and timing.
4. House rules: imports at the top of the file; `mod.rs` re-exports only; one primary export per file; at most 7 entries per folder and 1000 lines per source file for new/changed code; Fx hashers; no `std::sync::Mutex`/`RwLock` on hot paths; `Result` instead of panics; no private machine paths in committed files; terse comments.
5. Do not bump crate or dependency versions, do not add dependencies, do not touch CI workflows.
6. Run only the allowed local gates from the policy (clippy, fmt, layout) on the touched crates until clean.

## Final message (mandatory)

1. Per card: fixed / already-fixed / refuted, with the changed file:line and the regression test name.
2. Why each test fails on the old code (one sentence each).
3. Real exit codes of clippy, fmt --check and check_layout. Anything you could not verify.
