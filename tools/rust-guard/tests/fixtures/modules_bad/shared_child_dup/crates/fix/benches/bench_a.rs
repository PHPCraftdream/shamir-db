#[path = "common/helper.rs"]
mod helper;
pub fn a() -> u32 {
    helper::child()
}
