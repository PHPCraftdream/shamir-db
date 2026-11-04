use super::login_scope::LoginScope;
use crate::client::ApiToken;

/// Result of a successful password login: a reusable API token and its metadata.
#[non_exhaustive]
#[derive(Debug)]
pub struct LoginOutcome {
    pub token: ApiToken,
    pub id: [u8; 16],
    pub owner: String,
    pub scope: LoginScope,
    pub created_at_ns: u64,
    pub expires_at_ns: u64,
}
