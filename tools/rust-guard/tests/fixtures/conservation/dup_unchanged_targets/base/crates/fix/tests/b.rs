const N: u32 = 4;

fn helper() -> u32 {
    N
}

#[test]
fn works() {
    assert_eq!(helper(), 4);
}
