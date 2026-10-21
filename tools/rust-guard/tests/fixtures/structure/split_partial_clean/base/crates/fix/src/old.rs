pub fn keep() -> u32 {
    0
}

pub(crate) fn a() -> u32 {
    1
}

#[allow(dead_code)]
fn b() {}

pub fn c() -> u32 {
    // Function-local `use`, pre-existing in the base file.
    use std::fmt;
    fmt::format(format_args!("x"));
    2
}

#[cfg(test)]
mod tests {
    #[test]
    fn t() {
        assert_eq!(1, 1);
    }
}
