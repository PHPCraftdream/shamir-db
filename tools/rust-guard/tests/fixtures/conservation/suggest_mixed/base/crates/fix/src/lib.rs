pub fn helper(x: u32) -> u32 {
    x + 1
}

#[test]
fn t_ok() {
    assert_eq!(1 + 1, 2);
}

#[test]
fn t_mut() {
    assert_eq!(2 + 2, 4);
}

pub fn gone() -> u32 {
    7
}

pub fn dup() -> u32 {
    9
}

mod m;
