pub(crate) mod bridge;
pub(crate) mod decode_cache;
pub(crate) mod deliver_cache;
// `pub` (was `pub(crate)`): exposed for `benches/subscription_hot_paths.rs`
// to call `filter_eval::filter_matches_value` directly.
pub mod filter_eval;
// `pub` (was `pub(crate)`): exposed for `benches/subscription_hot_paths.rs`
// to call `payload::make_event_data` directly.
pub mod payload;
pub(crate) mod push;
pub(crate) mod reactive;
pub mod registry;
// `pub` (was `pub(crate)`): exposed for `benches/subscription_hot_paths.rs`
// to call `target_match::matches_any` directly.
pub mod target_match;

pub use registry::SubscriptionRegistry;

#[cfg(test)]
mod tests;
