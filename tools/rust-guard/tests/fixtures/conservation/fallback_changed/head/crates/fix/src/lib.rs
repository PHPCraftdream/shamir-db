#[cfg_attr(feature = "a", cfg(unix))]
#[cfg_attr(feature = "b", cfg_attr(feature = "c", path = "gen.rs"))]
mod m;

pub fn existing() -> u32 {
    1
}
