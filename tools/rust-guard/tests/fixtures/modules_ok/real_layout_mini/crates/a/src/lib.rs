//! Miniature of the real workspace layout: a package with a plain
//! module, a `#[path]`-mounted module, a bin target, an integration
//! test with a `#[path]` helper, a bench and a build script.
pub mod util;

#[path = "mounted.rs"]
mod mounted;

pub fn total() -> u32 {
    util::u() + mounted::m()
}
