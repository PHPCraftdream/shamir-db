//! Login route hook types and default disabled implementation.

mod login_head;
mod login_response;
mod login_routes;
mod no_login;
mod route_handler;
pub(crate) use route_handler::handle_login;
#[cfg(test)]
mod tests;

pub use login_head::LoginHead;
pub use login_response::LoginResponse;
pub use login_routes::LoginRoutes;
pub use no_login::NoLogin;
