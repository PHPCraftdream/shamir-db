#[path = "common/helper.rs"]
mod helper;
// Second logical mount of the SAME physical child.rs within THIS target.
#[path = "common/helper/child.rs"]
mod c2;
pub fn b() -> u32 {
    helper::child() + c2()
}
