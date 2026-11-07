//! Process-global built-in scalar function registry.
//!
//! `shamir-funclib` owns the function bodies AND the canonical
//! `static_builtin()` entry point; the engine re-exports it here for
//! backwards compatibility. The `ScalarResolver` (2-layer user + builtin)
//! lives in `shamir_funclib::scalar_resolver`.

use std::sync::OnceLock;

use shamir_funclib::agg::AggRegistry;
use shamir_funclib::registry::ScalarRegistry;

/// Return the shared built-in scalar registry, initialising it on first use.
///
/// The registry is immutable after construction and `Send + Sync`
/// (`FnEntry::f` is an `Arc<dyn Fn(..) + Send + Sync>`), so a `&'static`
/// reference can be handed to every concurrent query without locking.
///
/// Delegates to `shamir_funclib::static_builtin()` — the canonical home.
pub fn builtin_scalars() -> &'static ScalarRegistry {
    shamir_funclib::static_builtin()
}

/// Return the shared built-in aggregate registry, initialising it on first
/// use. Holds `AggFactory` closures (each `Send + Sync`); a fresh
/// `Box<dyn Aggregator>` is minted per GROUP-BY slot via
/// [`AggRegistry::make`](shamir_funclib::agg::AggRegistry::make).
pub fn builtin_aggs() -> &'static AggRegistry {
    static REG: OnceLock<AggRegistry> = OnceLock::new();
    REG.get_or_init(shamir_funclib::agg_builtins)
}
