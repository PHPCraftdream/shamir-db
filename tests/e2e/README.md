# tests/e2e

End-to-end test for the Node.js client binding (`shamir-client`,
crate at `crates/shamir-client-node/`). Drives a real `shamir-server`
subprocess through the full TLS 1.3 + SCRAM-Argon2id + Batch wire
flow.

## Requirements

Node **>=22.12**, enforced (`.npmrc`'s `engine-strict=true` in this
directory turns the `package.json` `engines` constraint into a hard
`npm install` failure — `EBADENGINE` — instead of a soft warning). This
suite's query-builder helpers `require()` `@shamir/client`
(`crates/shamir-client-ts`), an ESM-only package — Node 22.12+ can
`require()` an ESM module with no top-level await; older Node raises
`ERR_REQUIRE_ESM` at test-run time, which the enforced install-time
check now catches earlier and with a clearer message.

## One-time setup

```bash
cd tests/e2e
npm install
npm run build       # builds @shamir/client's dist/ + shamir-server release + .node binding
```

`npm run build` runs:

1. `cd crates/shamir-client-ts && npm ci && npm run build` —
   produces `crates/shamir-client-ts/dist/` (a gitignored artifact,
   `tsc -p configs/tsconfig.build.json`). `@shamir/client`'s own `devDependencies`
   (including `typescript`) are never installed by `tests/e2e`'s own
   `npm install` — npm does not install a `file:`-linked package's
   `devDependencies` — so this step's own `npm ci` (inside
   `crates/shamir-client-ts`, against that package's own committed
   `package-lock.json`) is what makes `tsc` resolvable. Without it,
   a fresh clone hits `Cannot find module '.../shamir-client-ts/dist/index.js'`.
2. `cargo build --release -p shamir-server` — produces `target/release/shamir-server[.exe]`
3. `cd crates/shamir-client-node && npm ci && npm run build` —
   `napi build --platform --release --output-dir native --js index.js
   --dts index.d.ts` produces
   `crates/shamir-client-node/native/shamir-client.<triple>.node`

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
`crates/shamir-e2e/tests/smoke.rs`.

## Connection matrix (`tests/21-connection-matrix.test.js`)

Starts its own second `shamir-server` (`helpers/matrix-server.js`) with six
listeners on fixed loopback ports, chosen from a base (default `14200`;
override with `SHAMIR_E2E_MATRIX_PORT_BASE`):

| Port | Listener |
|---|---|
| base+0 | `tcp`, `tls_exporter` |
| base+1 | `ws`, `tls_exporter`, path `/shamir/v1` |
| base+2 | `tcp`, `plain` (loopback only) |
| base+3 | `http`, TLS, password login enabled |
| base+4 | `http`, plaintext, password login enabled |
| — | `unix`, `plain` (Named Pipe on Windows, socket file in the tempdir elsewhere) |

It proves every connection variant of the napi binding: `connectWs`,
`connectPlain` (and its non-loopback refusal), `connectLocal`, `resume` /
`resumeWs`, API tokens through `ShamirTokenHttpClient` and `connectTokenWs`
(including the refusal of admin calls on token sessions), and password
`login`. The server is started by the first test and stopped by the last one.
