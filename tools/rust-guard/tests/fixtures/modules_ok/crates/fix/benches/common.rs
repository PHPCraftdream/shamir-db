// Shared bench helper: ALSO auto-discovered as a bench root of its
// own. Reaching this file from two TARGETS is normal cargo behaviour,
// never a DUPLICATE_FILE.
pub fn c() -> u32 {
    1
}
