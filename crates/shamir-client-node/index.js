// Native-binding loader. Resolves the prebuilt `.node` artefact for
// the current platform/arch/abi triple. Built artefacts follow the
// napi-rs naming convention: `shamir-client.<platform>-<arch>[-abi].node`.
//
// Today only `win32-x64-msvc` is built locally (see rust-toolchain.toml);
// add more triples here as we publish prebuilt binaries for other hosts.

'use strict';

const { platform, arch } = process;

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

module.exports = loadNative();
