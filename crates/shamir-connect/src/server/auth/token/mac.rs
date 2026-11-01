//! HMAC-SHA256 token authentication and constant-time tag comparison.
#[cfg(test)]
use super::verify::record_mac_computation;
use crate::server::auth::token::{TokenId, TokenSecret};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;
/// HMAC-SHA256 implementation used for token authentication.
type HmacSha256 = Hmac<Sha256>;
/// Computes the token MAC over its domain-separated fields.
pub fn compute_mac(
    key: &[u8; 32],
    key_id: u8,
    format_version: u8,
    id: &TokenId,
    secret: &TokenSecret,
) -> [u8; 32] {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts every key length");
    mac.update(b"shamir-api-token");
    mac.update(&[format_version, key_id]);
    mac.update(&id.0);
    mac.update(secret.as_bytes());
    let output: [u8; 32] = mac.finalize().into_bytes().into();
    #[cfg(test)]
    record_mac_computation();
    output
}
/// Compares MAC tags in constant time.
pub fn mac_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    bool::from(a.ct_eq(b))
}
/// Performs one dummy HMAC computation for an unknown token identifier.
pub fn dummy_mac_work(key: &[u8; 32], key_id: u8, format_version: u8) {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts every key length");
    mac.update(b"shamir-api-token");
    mac.update(&[format_version, key_id]);
    let dummy_id = [0u8; 16];
    let dummy_secret = [0u8; 32];
    mac.update(&dummy_id);
    mac.update(&dummy_secret);
    let output: [u8; 32] = mac.finalize().into_bytes().into();
    #[cfg(test)]
    record_mac_computation();
    std::hint::black_box(output);
}
