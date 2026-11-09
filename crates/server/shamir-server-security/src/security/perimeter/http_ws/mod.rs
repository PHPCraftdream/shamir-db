//! WebSocket security and admission policy for HTTP listeners.

pub(crate) mod browser;
pub(crate) mod cancel_registry;
mod route;
mod runtime;
#[cfg(test)]
mod tests;
mod upgrade;

pub use browser::policy::canonical_authority;
pub use browser::PreAuthAdmission;
pub use cancel_registry::{TokenWsCancelRegistry, TokenWsRegistration};
pub use route::{WsHeadState, WsSession, WsUpgradeService};
pub use runtime::{close_reason, TokenLoopContext, TokenWsRunner};
