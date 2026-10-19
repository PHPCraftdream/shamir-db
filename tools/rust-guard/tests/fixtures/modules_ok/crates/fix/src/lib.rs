//! Compiler-accepted module resolution shapes; the guard must stay
//! silent on this tree.

#[path = "sub/inner.rs"]
mod aliased;

// Mutually exclusive cfg_attr alternatives: BOTH are real, reachable
// files. The guard resolves every non-unsatisfiable alternative, so both
// `a.rs` and `b.rs` are accounted for with NO exception entry and no
// DUPLICATE_FILE (they are distinct physical files). The pair must be
// GENUINELY exclusive — `unix` vs `not(unix)`; a `feature="a"` /
// `feature="b"` pair is NOT (both features can be on or off) and would
// make the two branches a cfg-composed duplicate.
#[cfg_attr(unix, path = "a.rs")]
#[cfg_attr(not(unix), path = "b.rs")]
mod alt;

// BOTH cfg_attr branches are modelled: (feature "maybe" → gen.rs) and
// (not feature "maybe" → the DEFAULT candidates maybe.rs / maybe/mod.rs).
// Both branches resolve here, so both files are accounted without any
// exception and without DUPLICATE_FILE (the composed predicates
// `feature = "maybe"` and `not(feature = "maybe")` are exclusive).
#[cfg_attr(feature = "maybe", path = "gen.rs")]
mod maybe;

mod foo;
mod bar;

// Leading `./` and an interior `..` that lexically normalise to
// `sub/positive.rs`: resolution must normalise instead of using the
// un-normalised spelling as a literal path/visited key.
// Counterfactual: fails without lexical normalisation — the raw
// spelling was used as the lookup/visited key, `positive` was reported
// MISSING_MOD_FILE and the run was not clean.
#[path = "./sub/../sub/positive.rs"]
mod positive;

// A cfg_attr whose attr branch is MISSING but whose DEFAULT branch
// resolves: the missing attr alternative is an inactive branch, NOT a
// finding (fallback_ok.rs accounts the module).
// Counterfactual: pre-fix code reported MISSING_MOD_FILE for the
// inactive attr branch even though fallback_ok.rs resolved — this clean
// test failed.
#[cfg_attr(feature = "never_on", path = "missing_branch.rs")]
mod fallback_ok;

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
    aliased::inner() + alt::value() + foo::root() + bar::B + nested::leaf::v() + included()
}
