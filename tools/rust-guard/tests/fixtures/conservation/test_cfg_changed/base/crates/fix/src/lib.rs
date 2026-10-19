pub struct Plan {
    pub a: u32,
}

pub fn helper(x: u32) -> u32 {
    x + 1
}

// The whole inline test module's cfg differs between the revisions; the
// fn has no cfg of its own, so its INHERITED cfg text drifts v1 -> v2.
#[cfg(feature = "v1")]
mod m {
    #[test]
    fn t_cfg() {
        assert_eq!(1 + 1, 2);
    }
}
