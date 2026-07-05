// Native-binding loader + msgpack glue.
//
// Resolves the prebuilt `.node` artefact for the current platform/arch/abi
// triple. Built artefacts follow the napi-rs naming convention:
// `shamir-client.<platform>-<arch>[-abi].node`.
//
// The native `execute` / `repl` methods take and return MessagePack-encoded
// `Buffer`s (since the serde_json elimination refactor — see commit
// f19d593d). This loader wraps the native class so that JS callers can pass
// plain objects (the documented contract in `lib.rs` lines 19–22) and receive
// plain objects back: we encode on the way in, decode on the way out.
//
// Today only `win32-x64-msvc` is built locally (see rust-toolchain.toml);
// add more triples here as we publish prebuilt binaries for other hosts.

'use strict';

const { platform, arch } = process;
const { encode, decode } = require('@msgpack/msgpack');

function loadNative() {
  const candidates = [];

  if (platform === 'win32' && arch === 'x64') {
    candidates.push('./shamir-client.win32-x64-msvc.node');
    candidates.push('./shamir-client.win32-x64-gnu.node');
  } else if (platform === 'linux' && arch === 'x64') {
    candidates.push('./shamir-client.linux-x64-gnu.node');
    candidates.push('./shamir-client.linux-x64-musl.node');
  } else if (platform === 'linux' && arch === 'arm64') {
    candidates.push('./shamir-client.linux-arm64-gnu.node');
  } else if (platform === 'darwin' && arch === 'x64') {
    candidates.push('./shamir-client.darwin-x64.node');
  } else if (platform === 'darwin' && arch === 'arm64') {
    candidates.push('./shamir-client.darwin-arm64.node');
  } else {
    throw new Error(`Unsupported platform/arch: ${platform}/${arch}`);
  }

  let lastError;
  for (const path of candidates) {
    try {
      return require(path);
    } catch (e) {
      lastError = e;
    }
  }
  throw new Error(
    `Failed to load shamir-client native binding for ${platform}-${arch}.\n` +
      `Tried: ${candidates.join(', ')}\n` +
      `Last error: ${lastError && lastError.message}\n` +
      `Run \`npm run build\` in the package directory to compile it locally.`
  );
}

const native = loadNative();

// Wrap the native ShamirClient so `execute` accepts a plain JS object and
// returns a plain JS object (msgpack encode/decode happens here). `repl`
// already takes/returns Buffers in test 16, so we leave it untouched.
class ShamirClient extends native.ShamirClient {
  async execute(db, batchObj) {
    const buf = Buffer.from(encode(batchObj));
    const resp = await super.execute(db, buf);
    return decode(new Uint8Array(resp));
  }
}

module.exports = { ...native, ShamirClient };
