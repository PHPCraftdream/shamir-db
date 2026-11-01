//! Public token-core API; transport and persistence are deliberately absent.
pub mod format;
pub mod mac;
pub mod mirror;
pub mod record;
pub mod verify;

pub use format::{
    encode_token, generate_token_parts, parse_token, ParsedToken, TokenError, TokenId,
    TokenParseError, TokenSecret, ID_B64_LEN, SECRET_B64_LEN, TOKEN_LEN, TOKEN_PREFIX,
};
pub use mac::{compute_mac, dummy_mac_work, mac_eq};
pub use mirror::{TokenMirror, TokenState};
pub use record::{Endpoint, TokenChannels, TokenMode, TokenRecord};
pub use verify::{verify_token, OwnerFloors, Verified, VerifyCtx, VerifyFailure};
