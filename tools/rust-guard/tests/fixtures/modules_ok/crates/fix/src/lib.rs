//! Compiler-accepted module resolution shapes; the guard must stay
//! silent on this tree.

#[path = "sub/inner.rs"]
mod aliased;

// Mutually exclusive cfg_attr alternatives: only `a` is statically
// resolvable; `b` is covered by an exception entry.
#[cfg_attr(feature = "a", path = "a.rs")]
#[cfg_attr(feature = "b", path = "b.rs")]
mod alt;

mod foo;
mod bar;

// Inline module: extends the resolution directory context, so `leaf`
// resolves at `src/nested/leaf.rs`.
mod nested {
    pub mod leaf;
}

include!("included.rs");

// A raw string or comment containing a module declaration is NOT a
// declaration and must not produce findings:
const SNIPPET: &str = r#"
mod x;
"#;
// mod x;

pub fn call() -> u32 {
    aliased::inner() + alt::value() + foo::root() + bar::B + nested::leaf::v() + included()
}
