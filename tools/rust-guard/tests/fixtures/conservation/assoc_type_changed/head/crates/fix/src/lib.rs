pub trait T {
    type A;
}

pub struct S;

impl T for S {
    type A = u16;
}
