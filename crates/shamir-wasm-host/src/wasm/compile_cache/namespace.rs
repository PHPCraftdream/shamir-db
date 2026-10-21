use sha2::{Digest, Sha256};
use std::hash::{Hash, Hasher};
use wasmtime::Engine;

pub(crate) const FORMAT_VERSION: u32 = 1;

pub(crate) fn namespace(engine: &Engine) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"shamir-wasm-icache\0");
    hash.update(FORMAT_VERSION.to_le_bytes());
    hash.update(env!("CARGO_PKG_VERSION").as_bytes());
    let mut engine_hash = Sha256Hasher(hash);
    engine
        .precompile_compatibility_hash()
        .hash(&mut engine_hash);
    engine_hash.0.finalize().into()
}

struct Sha256Hasher(Sha256);

impl Hasher for Sha256Hasher {
    fn write(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }

    fn finish(&self) -> u64 {
        0
    }
}
