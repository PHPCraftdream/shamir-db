pub mod a {
    pub struct S;

    impl S {
        pub(super) fn f(&self) -> u32 {
            1
        }
    }
}

pub mod b {
    pub struct S;

    impl S {
        pub(super) fn f(&self) -> u32 {
            1
        }
    }
}
