//! WebSocket upgrade hook types.

pub(crate) mod handler;
mod hook;

pub use hook::{HeadOk, UpgradeHead, UpgradedIo, WsReject, WsRoute, WsUpgrade};
