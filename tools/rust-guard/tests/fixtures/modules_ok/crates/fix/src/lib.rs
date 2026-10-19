//! Compiler-accepted module resolution shapes; the guard must stay
//! silent on this tree.

#[path = "sub/inner.rs"]
mod aliased;

#[path = "a.rs"]
mod alt;

#[path = "b.rs"]
mod alternate;

mod foo;
mod bar;

// A plain inherited cfg is composed onto the child's reaching predicate.
#[cfg(unix)]
mod fallback_ok;

// Leading `./` and an interior `..` that lexically normalise to
// `sub/positive.rs`: resolution must normalise instead of using the
// un-normalised spelling as a literal path/visited key.
// Counterfactual: fails without lexical normalisation — the raw
// spelling was kept as the visited key, so the tracked canonical
// `sub/positive.rs` stayed UNACCOUNTED_FILE and the run was not clean.
#[path = "./sub/../sub/positive.rs"]
mod positive;

// Inline module: extends the resolution directory context, so `leaf`
// resolves at `src/nested/leaf.rs`.
mod nested {
    pub mod leaf;
}

// An INHERITED cfg gates the child: `leaf2` resolves at
// `src/gated/leaf2.rs` and carries the composed predicate `all(unix)` —
// the inline module's own cfg must AND into the file-backed child
// (finding 5a). The SAME physical file is reached a second time under
// `not(unix)`; the two edges are mutually exclusive ONLY when the first
// edge carries the composed inherited predicate, so `gated/leaf2.rs` is
// accounted exactly once with NO duplicate.
// Counterfactual: pre-fix the inline child's edge predicate was None
// (inherited cfg dropped), the two edges were not exclusive and the
// run reported DUPLICATE_FILE — this clean test failed.
#[cfg(unix)]
mod gated {
    pub mod leaf2;
}
#[cfg(not(unix))]
#[path = "gated/leaf2.rs"]
pub mod l2b;

include!("included.rs");

// Cross-TARGET sharing is not a duplicate: `benches/common.rs` is
// auto-discovered as a bench root of its own AND `mod common;`-included
// by `benches/bench_x.rs` — two different targets, one physical file.

// A raw string or comment containing a module declaration is NOT a
// declaration and must not produce findings:
const SNIPPET: &str = r#"
mod x;
"#;
// mod x;

pub fn call() -> u32 {
    aliased::inner() + alt::value() + alternate::value_b() + foo::root() + bar::B + nested::leaf::v() + included() + fallback_ok::fb()
}
