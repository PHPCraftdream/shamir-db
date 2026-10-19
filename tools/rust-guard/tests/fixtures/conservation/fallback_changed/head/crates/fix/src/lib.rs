#[cfg_attr(feature = "x", cfg_attr(feature = "y", path = "gen.rs"), cfg(unix))]
mod m;

pub fn existing() -> u32 {
    1
}
