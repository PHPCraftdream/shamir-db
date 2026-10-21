pub fn p() -> u32 {
    1
}

#[cfg(test)]
mod tests {
    #[test]
    fn t() {
        assert_eq!(1, 1);
    }
}
