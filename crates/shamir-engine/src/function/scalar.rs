//! Process-global built-in scalar function registry.
//!
//! `shamir-funclib` owns the function bodies; the engine exposes a single
//! lazily-initialised [`ScalarRegistry`] so the filter / set / group-by
//! paths can dispatch `FnCall`s by folder-qualified name (`"strings/upper"`,
//! `"math/abs"`). Building the registry runs one `register_builtins()` pass,
//! so it is built once on first use and shared for the process lifetime.

use std::sync::OnceLock;

use shamir_funclib::agg::AggRegistry;
use shamir_funclib::registry::ScalarRegistry;

/// Return the shared built-in scalar registry, initialising it on first use.
///
/// The registry is immutable after construction and `Send + Sync`
/// (`FnEntry::f` is an `Arc<dyn Fn(..) + Send + Sync>`), so a `&'static`
/// reference can be handed to every concurrent query without locking.
pub fn builtin_scalars() -> &'static ScalarRegistry {
    static REG: OnceLock<ScalarRegistry> = OnceLock::new();
    REG.get_or_init(shamir_funclib::register_builtins)
}

/// Return the shared built-in aggregate registry, initialising it on first
/// use. Holds `AggFactory` closures (each `Send + Sync`); a fresh
/// `Box<dyn Aggregator>` is minted per GROUP-BY slot via
/// [`AggRegistry::make`](shamir_funclib::agg::AggRegistry::make).
pub fn builtin_aggs() -> &'static AggRegistry {
    static REG: OnceLock<AggRegistry> = OnceLock::new();
    REG.get_or_init(shamir_funclib::agg_builtins)
}
