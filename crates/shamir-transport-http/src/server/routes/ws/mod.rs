//! WebSocket upgrade hook types.

pub(crate) mod handler;
mod hook;

pub use hook::{HeadOk, UpgradeHead, UpgradedIo, WsAdmission, WsReject, WsRoute, WsUpgrade};
