# tests/e2e

End-to-end test for the Node.js client binding (`shamir-client`,
crate at `crates/shamir-client-node/`). Drives a real `shamir-server`
subprocess through the full TLS 1.3 + SCRAM-Argon2id + Batch wire
flow.

## One-time setup

```bash
cd tests/e2e
npm install
npm run build       # builds shamir-server release + .node binding
```

`npm run build` runs:

1. `cargo build --release -p shamir-server` — produces `target/release/shamir-server[.exe]`
2. `napi build --platform --release` — produces `crates/shamir-client-node/shamir-client.<triple>.node`

The binding is published locally via `file:` reference in
`package.json`, so `npm install` symlinks it directly.

## Run

```bash
npm test
```

What it does:

1. Creates a tempdir + a minimal `server.ktav` config (TCP+TLS, fast Argon2id).
2. Spawns `shamir-server` with `--bootstrap-password admin` against the tempdir.
3. Waits for the listener to bind (parses tracing log line).
4. Connects via `ShamirClient.connect(...)` — full SCRAM handshake,
   TOFU pin capture.
5. Exercises: `ping` → `create_db` → `create_repo` + `create_table` →
   `set` + `from` (single batch).
6. Closes the client, kills the server, cleans the tempdir.

## What this proves

- The native binding loads on the host platform.
- The Rust SDK's TLS+SCRAM handshake interoperates with a real server
  binary (not just an in-process `ServerLauncher` test).
- BatchRequest/BatchResponse round-trip cleanly across the napi/JS
  boundary (MessagePack encoding).
- The release-mode server is functional end-to-end.

For pure-Rust integration tests (in-process `ServerLauncher`, no
subprocess) see `crates/shamir-server/tests/mvp_e2e.rs` and
`crates/shamir-client/tests/smoke.rs`.
