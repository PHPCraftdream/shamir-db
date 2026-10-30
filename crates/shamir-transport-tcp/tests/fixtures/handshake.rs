use serde::{Deserialize, Serialize};
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::scram::DerivedKeys;
use shamir_connect::common::username::NormalizedUsername;
use shamir_connect::server::user_record::UserRecord;
use zeroize::Zeroizing;

#[derive(Serialize, Deserialize)]
pub struct WireAuthInit {
    pub user: String,
    #[serde(with = "serde_bytes")]
    pub client_nonce: Vec<u8>, // 32 bytes
    pub binding_mode: u8,
    pub version: u8,
}

#[derive(Serialize, Deserialize)]
pub struct WireChallenge {
    #[serde(with = "serde_bytes")]
    pub salt: Vec<u8>, // 16 bytes
    pub memory_kb: u32,
    pub time: u32,
    pub parallelism: u32,
    pub argon2_version: u8,
    #[serde(with = "serde_bytes")]
    pub server_nonce: Vec<u8>, // 32 bytes
}

#[derive(Serialize, Deserialize)]
pub struct WireClientProof {
    #[serde(with = "serde_bytes")]
    pub client_proof: Vec<u8>, // 32 bytes
}

#[derive(Serialize, Deserialize)]
pub struct WireAuthOk {
    #[serde(with = "serde_bytes")]
    pub server_signature: Vec<u8>, // 32
    #[serde(with = "serde_bytes")]
    pub server_pub_key: Vec<u8>, // 32
    #[serde(with = "serde_bytes")]
    pub identity_sig: Vec<u8>, // 64
    #[serde(with = "serde_bytes")]
    pub session_id: Vec<u8>, // 32
    pub expires_at_ns: u64,
}

pub fn fast_kdf() -> KdfParams {
    // Faster than the spec floor — deliberate for test runtime. Tests must
    // pass `validate_client_limits` on the client side.
    KdfParams {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    }
}

pub fn make_user(password: &[u8]) -> (UserRecord, NormalizedUsername) {
    let username = NormalizedUsername::from_raw("alice").unwrap();
    let salt = [0x42u8; 16];
    let kdf = fast_kdf();
    let derived = DerivedKeys::derive(password, &salt, &kdf).unwrap();
    let mut server_key_z: Zeroizing<[u8; 32]> = Zeroizing::new([0u8; 32]);
    server_key_z.copy_from_slice(&derived.server_key[..]);
    let record = UserRecord {
        salt,
        stored_key: derived.stored_key,
        server_key: server_key_z,
        kdf_params: kdf,
        tickets_invalid_before_ns: 0,
    };
    (record, username)
}
