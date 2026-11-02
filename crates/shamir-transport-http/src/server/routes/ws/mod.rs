//! WebSocket upgrade hook types.

mod hook;

pub use hook::{HeadOk, UpgradeHead, UpgradedIo, WsReject, WsRoute, WsUpgrade};
