const N: u32 = 3;

fn helper() -> u32 {
    N
}

#[test]
fn works() {
    assert_eq!(helper(), 3);
}
