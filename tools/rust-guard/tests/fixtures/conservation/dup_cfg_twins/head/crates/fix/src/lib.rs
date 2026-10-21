// Only this comment changed: the cfg twins keep one logical path.
#[cfg(unix)]
pub fn plat() -> u32 {
    1
}

#[cfg(windows)]
pub fn plat() -> u32 {
    2
}

#[cfg(feature = "x")]
pub fn hook(phase: &str) -> usize {
    phase.len()
}

#[cfg(not(feature = "x"))]
pub fn hook(_phase: &str) -> usize {
    0
}
