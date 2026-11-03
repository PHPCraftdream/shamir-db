//! Login route extension points for the HTTP transport.

mod login;

pub(crate) use login::handle_login;
pub use login::{LoginHead, LoginResponse, LoginRoutes, NoLogin};
