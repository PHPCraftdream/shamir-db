//! Public token-core API; transport and persistence are deliberately absent.
/// Canonical token encoding and parsing types.
pub mod format;
/// Token authentication MAC operations.
pub mod mac;
/// In-memory token state mirror.
pub mod mirror;
/// Token records and authorization scope types.
pub mod record;
/// Token verification and owner-floor contracts.
pub mod verify;

pub use format::{
    encode_token, generate_token_parts, parse_token, ParsedToken, TokenError, TokenId,
    TokenParseError, TokenSecret, ID_B64_LEN, SECRET_B64_LEN, TOKEN_LEN, TOKEN_PREFIX,
};
pub use mac::{compute_mac, dummy_mac_work, mac_eq};
pub use mirror::{TokenMirror, TokenState};
pub use record::{Endpoint, TokenChannels, TokenMode, TokenRecord};
pub use verify::{verify_token, OwnerFloors, Verified, VerifyCtx, VerifyFailure};
