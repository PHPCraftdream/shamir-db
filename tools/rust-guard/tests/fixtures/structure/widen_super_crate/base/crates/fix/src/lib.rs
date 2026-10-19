// Base: `helper` confined to an ancestor module (`pub(super)`).
pub mod m {
    pub(super) fn helper() -> u32 {
        1
    }
}
