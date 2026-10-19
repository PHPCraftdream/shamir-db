#[path = "t/helper.rs"]
mod helper;

#[test]
fn uses_helper() {
    assert_eq!(helper::h(), 3);
}
