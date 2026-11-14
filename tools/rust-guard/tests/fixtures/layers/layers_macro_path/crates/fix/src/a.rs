//! Layer a (low): the only upward reference lives in macro tokens.

macro_rules! take {
    ($t:ty) => {
        fn takes() -> Option<$t> {
            None
        }
    };
}

take!(crate::b::Thing);
