// An INLINE module in a mod.rs must also hold only manifest items:
// logic hidden in `mod inner` is a MODRS_LOGIC finding (finding 10f).
pub mod inner {
    pub fn f() -> u32 {
        1
    }
}
