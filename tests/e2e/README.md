# tests/e2e

End-to-end test for the Node.js client binding (`shamir-client`,
crate at `crates/shamir-client-node/`). Drives a real `shamir-server`
subprocess through the full TLS 1.3 + SCRAM-Argon2id + Batch wire
flow.

## Requirements

Node **>=22.12** (declared in `package.json`'s `engines`). This suite's
query-builder helpers `require()` `@shamir/client` (`crates/shamir-client-ts`),
an ESM-only package — Node 22.12+ can `require()` an ESM module with no
top-level await; older Node raises `ERR_REQUIRE_ESM`.

## One-time setup

```bash
cd tests/e2e
npm install
npm run build       # builds @shamir/client's dist/ + shamir-server release + .node binding
```

`npm run build` runs:

1. `cd crates/shamir-client-ts && npm ci && npm run build` —
   produces `crates/shamir-client-ts/dist/` (a gitignored artifact,
   `tsc -p tsconfig.build.json`). `@shamir/client`'s own `devDependencies`
   (including `typescript`) are never installed by `tests/e2e`'s own
   `npm install` — npm does not install a `file:`-linked package's
   `devDependencies` — so this step's own `npm ci` (inside
   `crates/shamir-client-ts`, against that package's own committed
   `package-lock.json`) is what makes `tsc` resolvable. Without it,
   a fresh clone hits `Cannot find module '.../shamir-client-ts/dist/index.js'`.
2. `cargo build --release -p shamir-server` — produces `target/release/shamir-server[.exe]`
3. `napi build --platform --release` — produces `crates/shamir-client-node/shamir-client.<triple>.node`

`shamir-client` (the napi binding) and `@shamir/client` (a pure-TypeScript
package, no native code) are both published locally via `file:`
references in `package.json`, so `tests/e2e`'s own `npm install` symlinks
both directly — but only `shamir-client`'s native binary and
`@shamir/client`'s `dist/` need a build step first; the `file:` symlink
alone isn't enough for either.

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
