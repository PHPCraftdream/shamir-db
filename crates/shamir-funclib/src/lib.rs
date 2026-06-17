//! `shamir-funclib` — ShamirDB's built-in scalar function library.
//!
//! A single [`registry::ScalarRegistry`] maps folder-qualified function names
//! (`"math/abs"`, `"strings/lower"`, `"arrays/min"`) to pure
//! `fn(&[QueryValue]) -> ScalarResult`. [`register_builtins`] wires each
//! category module via [`ScalarRegistry::in_folder`](registry::ScalarRegistry::in_folder)
//! so that categories sharing a plain name (`json/keys` vs `object/keys`,
//! `math/min` vs `arrays/min`) no longer collide.
//!
//! Each category lives in its own module exposing `pub fn register(&mut
//! ScalarRegistry)`. [`register_builtins`] wires them all into one registry.
//! [`math`] is the fully-implemented reference; the remaining categories are
//! stubs to be populated by their owning agents.
//!
//! # QueryValue ABI — #61 InnerValue-elimination campaign
//!
//! funclib's scalar/aggregate ABI operates on `QueryValue` (string-keyed
//! `Value<String>`). For scalar leaves (Int/Str/Bool/Dec/F64/Bin) the
//! key type is irrelevant; for the handful of Map/List/Set-producing
//! functions, string keys are MORE natural (no interner needed). Callers
//! at the engine/wasm-host boundary convert to/from `InnerValue` as needed.

pub mod registry;

pub mod agg;
pub mod compare;

pub mod arrays;
pub mod canonical;
pub mod cast;
pub mod crypto;
pub mod datetime;
pub mod encode;
pub mod json;
pub mod math;
pub mod object;
pub mod strings;
pub mod text;
pub mod validate;

/// Build a registry populated with every built-in category.
///
/// Each category is registered under a folder prefix (`math/abs`, `json/keys`,
/// `object/keys`, etc.) so that categories sharing a plain name no longer
/// collide. See [`registry::ScalarRegistry::in_folder`].
pub fn register_builtins() -> registry::ScalarRegistry {
    let mut reg = registry::ScalarRegistry::new();
    reg.in_folder("math", math::register);
    reg.in_folder("strings", strings::register);
    reg.in_folder("arrays", arrays::register);
    reg.in_folder("cast", cast::register);
    reg.in_folder("datetime", datetime::register);
    reg.in_folder("json", json::register);
    reg.in_folder("validate", validate::register);
    reg.in_folder("encode", encode::register);
    reg.in_folder("object", object::register);
    reg.in_folder("text", text::register);
    reg.in_folder("crypto", crypto::register);
    reg.in_folder("crypto", canonical::register);
    reg
}

/// Build an [`agg::AggRegistry`] populated with every built-in aggregate.
pub fn agg_builtins() -> agg::AggRegistry {
    let mut r = agg::AggRegistry::new();
    agg::register(&mut r);
    r
}

#[cfg(test)]
mod tests;
