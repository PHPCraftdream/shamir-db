#[cfg_attr(all(feature = "x", any(unix, windows)), path = "gen.rs")]
mod m;

pub fn existing() -> u32 {
    1
}
