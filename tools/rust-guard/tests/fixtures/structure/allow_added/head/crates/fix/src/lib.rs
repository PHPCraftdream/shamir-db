pub fn existing() -> u32 {
    1
}

// The `#[allow]` arrives between the revisions: ALLOW_ADDED, suppressed
// only by a matching `[[allow]]` delta entry.
#[allow(dead_code)]
fn helper() -> u32 {
    2
}
