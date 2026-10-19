//! Same shape as modules_ok/producer_declared, but the [[producer]] is
//! scoped to a DIFFERENT package — package-scoped lookup must refuse to
//! let it suppress the macro finding here.

macro_rules! declare_mods {
    () => {
        mod generated;
    };
}
declare_mods!();

pub fn existing() -> u32 {
    1
}
