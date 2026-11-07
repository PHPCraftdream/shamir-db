//! WebSocket security and admission policy for HTTP listeners.

pub(crate) mod browser;
pub(crate) mod cancel_registry;
mod route;
#[cfg(test)]
mod tests;
mod upgrade;

pub(crate) use browser::PreAuthAdmission;
pub(crate) use route::WsUpgradeService;
